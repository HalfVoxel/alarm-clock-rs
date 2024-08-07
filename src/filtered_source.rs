use rodio::{Sample, Source};

use std::{sync::Arc, sync::Mutex, time};
use synthrs::filter::{cutoff_from_frequency, lowpass_filter};
use time::Duration;

/// Internal function that builds a `FilteredSource` object.
pub fn dynamic_filter<I>(
    input: I,
    lowpass_freq: Box<dyn Fn(f64) -> f64 + Send + Sync>,
) -> (FilteredSource<I>, Controller)
where
    I: Source<Item = f32>,
{
    let sample_rate = input.sample_rate();
    let source = FilteredSource {
        input,
        settings: Arc::new(Mutex::new(Settings {
            lowpass: vec![],
            volume: 1.0,
        })),
        current_buffer: vec![],
        current_buffer_index: 0,
        input_buffer: vec![],
        trailing_samples: vec![],
        lowpass_freq,
        sample_count: 0,
        last_lowpass_recalculation: 0,
    };

    let controller = Controller {
        sample_rate,
        settings: source.settings.clone(),
    };

    //controller.set_lowpass(5000.0);

    (source, controller)
}

pub struct Settings {
    lowpass: Vec<f32>,
    volume: f32,
}

/// Filter that modifies reduces the volume to silence over a time period.
pub struct FilteredSource<I> {
    input: I,
    input_buffer: Vec<f32>,
    settings: Arc<Mutex<Settings>>,
    trailing_samples: Vec<f32>,
    current_buffer_index: usize,
    current_buffer: Vec<f32>,
    lowpass_freq: Box<dyn Fn(f64) -> f64 + Send + Sync>,
    sample_count: usize,
    last_lowpass_recalculation: usize,
}

pub struct Controller {
    #[allow(unused)]
    sample_rate: u32,
    settings: Arc<Mutex<Settings>>,
}

impl Controller {
    pub fn set_volume(&self, v: f32) {
        self.settings.lock().unwrap().volume = v;
    }
}

#[allow(unused)]
pub fn convolve(filter: &[f32], input: &[f32], output: &mut [f32]) {
    assert_eq!(output.len(), input.len() - filter.len(), "output size are only the inner valid samples. filter.len()/2 samples on each side are skipped.");
    assert_eq!(filter.len() % 2, 0, "filter must have an even length");
    assert!(
        input.len() >= filter.len(),
        "input must be at least as long as filter"
    );

    let h_len = filter.len() / 2;
    for i in h_len..input.len() - h_len {
        let mut v = 0.0;
        for j in 0..filter.len() {
            v += input[i + j - h_len] * filter[j];
        }
        output[i - h_len] = v;
    }
}

#[allow(unused)]
pub fn convolve_f64(filter: &[f64], input: &[f64], output: &mut [f64]) {
    assert_eq!(output.len(), input.len() - filter.len(), "output size are only the inner valid samples. filter.len()/2 samples on each side are skipped.");
    assert_eq!(filter.len() % 2, 0, "filter must have an even length");
    assert!(
        input.len() >= filter.len(),
        "input must be at least as long as filter"
    );

    let h_len = filter.len() / 2;
    for i in h_len..input.len() - h_len {
        let mut v = 0.0;
        for j in 0..filter.len() {
            v += input[i + j - h_len] * filter[j];
        }
        output[i - h_len] = v;
    }
}

impl<I> Iterator for FilteredSource<I>
where
    I: Source<Item = f32>,
    I::Item: Sample,
{
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.current_buffer_index < self.current_buffer.len() {
            self.current_buffer_index += 1;
            self.sample_count += 1;
            return Some(self.current_buffer[self.current_buffer_index - 1]);
        }

        let t = self.sample_count as f64 / (self.channels() as f64 * self.sample_rate() as f64);

        {
            let mut settings = self.settings.lock().unwrap();
            let lowpass = &mut settings.lowpass;

            if lowpass.is_empty() || self.sample_count > self.last_lowpass_recalculation + 8192 {
                self.last_lowpass_recalculation = self.sample_count;
                let freq = (self.lowpass_freq)(t);
                let lowpass64 = lowpass_filter(
                    cutoff_from_frequency(
                        freq.min((self.sample_rate() / 2) as f64),
                        self.sample_rate() as usize,
                    ),
                    0.01,
                );
                *lowpass = lowpass64.iter().map(|&x| x as f32).collect();
            }

            // Must be at least the same size as the filter
            let frame_size = 1024 - lowpass.len();

            let input_samples = &mut self.input_buffer;
            input_samples.clear();
            input_samples.append(&mut self.trailing_samples);
            input_samples.extend(
                self.input
                    .by_ref()
                    .chain(std::iter::repeat(0.0))
                    .take(frame_size),
            );

            assert!(
                input_samples.len() >= lowpass.len(),
                "{} >= {}",
                input_samples.len(),
                lowpass.len()
            );

            self.trailing_samples
                .extend_from_slice(&input_samples[input_samples.len() - lowpass.len()..]);

            let buffer = &mut self.current_buffer;
            buffer.resize(input_samples.len() - lowpass.len(), 0.0);
            convolve(lowpass, input_samples, buffer);

            for s in buffer {
                *s *= settings.volume;
                *s = s.clamp(-1.0, 1.0);
            }

            self.current_buffer_index = 0;
        }

        self.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let inner = self.input.size_hint();
        (
            inner.0 + self.current_buffer.len(),
            inner.1.map(|x| x + self.current_buffer.len()),
        )
    }
}

impl<I> ExactSizeIterator for FilteredSource<I>
where
    I: Source<Item = f32> + ExactSizeIterator,
    I::Item: Sample,
{
}

impl<I> Source for FilteredSource<I>
where
    I: Source<Item = f32>,
    I::Item: Sample,
{
    #[inline]
    fn current_frame_len(&self) -> Option<usize> {
        self.input
            .current_frame_len()
            .map(|x| x + self.current_buffer.len())
    }

    #[inline]
    fn channels(&self) -> u16 {
        self.input.channels()
    }

    #[inline]
    fn sample_rate(&self) -> u32 {
        self.input.sample_rate()
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }
}
