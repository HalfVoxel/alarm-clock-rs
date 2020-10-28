use rodio::{Sample, Source};

use std::{time};
use time::Duration;

#[derive(Clone)]
pub struct PrecalculatedSource<I> {
    input: I,
    current_buffer: Vec<f32>,
    current_buffer_index: usize,
}

impl<I> PrecalculatedSource<I>
where
    I: Source<Item = f32>,
    I::Item: Sample,
{
    pub fn new(input: I, samples_to_precalculate: usize) -> Self {
        let mut r = Self {
            input,
            current_buffer: vec![],
            current_buffer_index: 0,
        };
        r.precalculate(samples_to_precalculate);
        r
    }

    pub fn precalculate(&mut self, samples_to_precalculate: usize) {
        println!("Precalculating {} samples", samples_to_precalculate);
        self.current_buffer.extend(self.input.by_ref().take(samples_to_precalculate));
        println!("Precalculation done. Got {} samples", self.current_buffer.len());
    }
}

impl<I> Iterator for PrecalculatedSource<I>
where
    I: Source<Item = f32>,
    I::Item: Sample,
{
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.current_buffer_index < self.current_buffer.len() {
            self.current_buffer_index += 1;
            return Some(self.current_buffer[self.current_buffer_index - 1]);
        } else {
            if !self.current_buffer.is_empty() {
                println!("Ran out of precalculated samples. Fetching dynamically instead.");
                self.current_buffer.clear();
                self.current_buffer_index = 0;
            }
            self.input.next()
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let inner = self.input.size_hint();
        (
            inner.0 + self.current_buffer.len() - self.current_buffer_index,
            inner.1.map(|x| x + self.current_buffer.len() - self.current_buffer_index),
        )
    }
}

impl<I> ExactSizeIterator for PrecalculatedSource<I>
where
    I: Source<Item = f32> + ExactSizeIterator,
    I::Item: Sample,
{
}

impl<I> Source for PrecalculatedSource<I>
where
    I: Source<Item = f32>,
    I::Item: Sample,
{
    #[inline]
    fn current_frame_len(&self) -> Option<usize> {
        self.input
            .current_frame_len()
            .map(|x| x + self.current_buffer.len() - self.current_buffer_index)
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
