//! Small seeded randomness for the agent's creative picks (which sound,
//! which track, which look): each video gets a fresh seed, so two videos
//! made from the same script still differ, while one seed always gives the
//! same picks (tests, and a scene's file named by what made it).

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Dice(u64);

impl Dice {
    pub fn new(seed: u64) -> Dice {
        // splitmix64 of the seed: nearby seeds start far apart.
        let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Dice((z ^ (z >> 31)) | 1)
    }

    /// A seed nobody asked for: the clock's nanoseconds and the process.
    pub fn fresh_seed() -> u64 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        nanos ^ ((std::process::id() as u64) << 32)
    }

    /// A seed for one part of a whole (a cue kind, a scene's index).
    pub fn mix(seed: u64, part: u64) -> u64 {
        Dice::new(seed ^ part.wrapping_mul(0x9e37_79b9_7f4a_7c15)).next()
    }

    pub fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// 0 ≤ n < `below` (0 when `below` is 0).
    pub fn below(&mut self, below: usize) -> usize {
        if below == 0 {
            0
        } else {
            (self.next() % below as u64) as usize
        }
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_picks_and_spread() {
        let mut a = Dice::new(7);
        let mut b = Dice::new(7);
        let xs: Vec<usize> = (0..20).map(|_| a.below(10)).collect();
        let ys: Vec<usize> = (0..20).map(|_| b.below(10)).collect();
        assert_eq!(xs, ys);
        let mut c = Dice::new(8);
        assert_ne!(xs, (0..20).map(|_| c.below(10)).collect::<Vec<_>>());
        let mut v: Vec<u32> = (0..10).collect();
        Dice::new(3).shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..10).collect::<Vec<_>>());
    }
}
