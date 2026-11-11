//! Tiny deterministic generator. A non-zero xorshift64* state never collapses
//! to zero, so a campaign can be replayed from its seed alone.

#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let state = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        Self {
            state: if state == 0 { 1 } else { state },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0..n`. `n == 0` yields 0 so a caller can still make progress.
    pub fn gen_range(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() as usize) % n
        }
    }

    pub fn chance(&mut self, numer: u32, denom: u32) -> bool {
        if denom == 0 {
            return false;
        }
        self.next_u64() % u64::from(denom) < u64::from(numer)
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.gen_range(items.len())]
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn the_same_seed_repeats_and_a_different_seed_does_not() {
        let mut a = Rng::new(13);
        let mut b = Rng::new(13);
        let mut c = Rng::new(14);
        let left: Vec<u64> = (0..32).map(|_| a.next_u64()).collect();
        let right: Vec<u64> = (0..32).map(|_| b.next_u64()).collect();
        let other: Vec<u64> = (0..32).map(|_| c.next_u64()).collect();
        assert_eq!(left, right);
        assert_ne!(left, other);
    }

    #[test]
    fn gen_range_stays_inside_the_interval() {
        let mut rng = Rng::new(1);
        for _ in 0..1000 {
            assert!(rng.gen_range(7) < 7);
        }
        assert_eq!(rng.gen_range(0), 0);
    }
}
