//! 一个够用的伪随机数发生器。
//!
//! 刻意不引第三方 `rand`：这里只需要「同一种子必然复现同一序列」，
//! 好让单测能钉死分布和取值边界。`rand` 默认从系统熵取种子，
//! 测「抽一万次里 idle 占多少」还得先想法子固定种子，多绕一圈。
//!
//! 用的是 xorshift64：周期 2^64-1，状态只有一个 `u64`，
//! 快到可以忽略——桌宠每秒最多抽一次。

/// 64 位 xorshift。
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// 用指定种子新建。种子为 0 时换成一个魔数：
    /// 0 是 xorshift 的固定点，喂 0 进去会永远吐出 0。
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// 用当前系统时间做种子。生产路径用它，测试路径用 [`Rng::new`]。
    pub fn from_time() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5DEE_CE66_D1CE_B00F);
        Self::new(nanos)
    }

    /// 下一个 64 位随机数。
    pub fn next_u64(&mut self) -> u64 {
        // xorshift64 的移位与异或组合，全在 u64 内，无溢出风险。
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// `[0, 1)` 的浮点数。
    ///
    /// 取高 24 位而不是低位：xorshift 的低位周期性更明显，
    /// 而 24 位正好是 `f32` 的尾数宽度，除出来每个值都精确可表示。
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// `[lo, hi)` 的浮点数。`lo > hi` 时自动交换，不会返回 NaN。
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let (a, b) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        a + (b - a) * self.unit()
    }

    /// `[0, n)` 的整数。`n == 0` 时返回 0（调用方不必先判空）。
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % u64::from(n)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(12345);
        let mut b = Rng::new(12345);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        let same = (0..100).filter(|_| a.next_u64() == b.next_u64()).count();
        assert!(same < 3, "不同种子不该产生几乎相同的序列（撞了 {same} 次）");
    }

    #[test]
    fn zero_seed_does_not_stick() {
        let mut r = Rng::new(0);
        let first = r.next_u64();
        assert_ne!(first, 0, "种子 0 必须被换掉，否则永远吐 0");
        assert_ne!(r.next_u64(), first);
    }

    #[test]
    fn unit_stays_in_range() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let v = r.unit();
            assert!((0.0..1.0).contains(&v), "unit() 越界：{v}");
        }
    }

    #[test]
    fn range_respects_bounds_and_order() {
        let mut r = Rng::new(99);
        for _ in 0..10_000 {
            let v = r.range(18.0, 38.0);
            assert!((18.0..38.0).contains(&v), "range 越界：{v}");
            // 反着传也要落在同一区间内。
            let w = r.range(38.0, 18.0);
            assert!((18.0..38.0).contains(&w), "倒序 range 越界：{w}");
        }
    }

    #[test]
    fn below_stays_under_n() {
        let mut r = Rng::new(2024);
        for _ in 0..10_000 {
            assert!(r.below(7) < 7);
        }
        assert_eq!(r.below(1), 0, "上界 1 只有 0");
        assert_eq!(r.below(0), 0, "上界 0 不该 panic");
    }

    /// 分布不能一边倒：抽 10000 次落在 4 个桶里，每桶都该见到。
    #[test]
    fn below_is_roughly_uniform() {
        let mut r = Rng::new(5150);
        let mut buckets = [0u32; 4];
        for _ in 0..10_000 {
            buckets[r.below(4) as usize] += 1;
        }
        for (i, n) in buckets.iter().enumerate() {
            assert!(
                (2200..2800).contains(n),
                "第 {i} 桶 {n} 次，偏离 2500 太多：{buckets:?}"
            );
        }
    }
}
