use std::{fmt::Display, time::Duration};

#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[cfg(target_arch = "wasm32")]
use wasm_timer::Instant;

pub struct Timer {
    start: Instant,
    rounds: Vec<(Instant, String)>,
}

impl Timer {
    pub(crate) fn round(&mut self, desc: String) {
        self.rounds.push((Instant::now(), desc));
    }

    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            rounds: vec![],
        }
    }

    pub(crate) fn instants(&self) -> impl Iterator<Item = Instant> {
        self.rounds.iter().map(|&(i, _)| i)
    }

    pub(crate) fn descriptions(&self) -> impl Iterator<Item = String> {
        self.rounds.iter().cloned().map(|(_, d)| d)
    }

    pub(crate) fn durations(&self) -> impl Iterator<Item = Duration> {
        self.instants()
            .zip(std::iter::once(self.start).chain(self.instants()))
            .map(|(a, b)| a - b)
    }

    pub(crate) fn category(&self, desc: String) -> Duration {
        self.durations()
            .zip(self.descriptions())
            .filter(|(_, d)| *d == desc)
            .map(|(r, _)| r)
            .sum()
    }

    pub(crate) fn total(&self) -> Duration {
        self.rounds.last().map(|&(i, _)| i).unwrap_or(self.start) - self.start
    }
}

impl Display for Timer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (dur, desc) in self.durations().zip(self.descriptions()) {
            writeln!(f, "{dur:>10?} {desc}")?;
        }
        writeln!(f, "total: {:>10?}", self.total())?;
        Ok(())
    }
}
