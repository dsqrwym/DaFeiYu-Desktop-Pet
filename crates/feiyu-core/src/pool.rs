//! 待机小动作的权重池。
//!
//! 桌宠一直播 idle 会显得死板。这里给它一点自娱自乐：够无聊了就抽一段
//! 小动作播一遍，播完回 idle。
//!
//! 用**权重表**而不是「等概率列表」，是为了二期加 `turn`/`move` 时
//! 只要往表里追加数据，不用改抽取逻辑，也不用重新调比例。
//!
//! 权重刻着原版的比例（`dsh-pet` 的 `animationWeights`：idle 10、turn 5、
//! move 5、categories 20）。一期只上 categories 里拿得到的那两段，
//! 所以表里现在只有两项等权——等二期把 `turn`/`move` 补进来，
//! 追加时照原版填数即可。

use crate::rng::Rng;

/// 权重表：抽中某个 id 的相对概率。
#[derive(Debug, Clone, Default)]
pub struct Pool {
    items: Vec<(&'static str, u32)>,
    total: u32,
}

impl Pool {
    /// 按给定顺序建池。权重为 0 的项会被丢掉。
    pub fn new(items: impl IntoIterator<Item = (&'static str, u32)>) -> Self {
        let mut pool = Self::default();
        for (id, weight) in items {
            pool.add(id, weight);
        }
        pool
    }

    /// 追加一项。
    ///
    /// 权重 0 或 id 重复都直接忽略：重复会让那一段的实际概率悄悄翻倍，
    /// 而调用方以为自己只写了一行。
    pub fn add(&mut self, id: &'static str, weight: u32) {
        if weight == 0 || self.items.iter().any(|(x, _)| *x == id) {
            return;
        }
        self.items.push((id, weight));
        self.total = self.total.saturating_add(weight);
    }

    /// 只留下 `available` 里存在的 id。
    ///
    /// 素材缺段时池子会自动瘦身，而不是抽到一个打不开的文件再报错——
    /// 「少一段小动作」和「桌宠崩了」之间差得很远。
    pub fn retained(mut self, available: &[&str]) -> Self {
        self.items.retain(|(id, _)| available.contains(id));
        self.total = self.items.iter().map(|(_, w)| *w).sum();
        self
    }

    /// 池子里还有没有东西可抽。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 池子里剩几项。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 权重总和。
    pub fn total_weight(&self) -> u32 {
        self.total
    }

    /// 当前池内容 `(id, 权重)`。
    pub fn items(&self) -> &[(&'static str, u32)] {
        &self.items
    }

    /// 按权重抽一个。空池返回 `None`（调用方据此决定"这次不播"）。
    pub fn pick(&self, rng: &mut Rng) -> Option<&'static str> {
        if self.total == 0 {
            return None;
        }
        let mut r = rng.below(self.total);
        for (id, weight) in &self.items {
            if r < *weight {
                return Some(id);
            }
            r -= *weight;
        }
        // 理论上走不到（r 一定小于 total）。兜底给最后一项而不是 panic：
        // 为一次抽取失手而让桌宠崩掉不值得。
        self.items.last().map(|(id, _)| *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_ids(pool: &Pool) -> Vec<&'static str> {
        pool.items().iter().map(|(id, _)| *id).collect()
    }

    #[test]
    fn empty_pool_picks_nothing() {
        let pool = Pool::new(std::iter::empty());
        assert!(pool.is_empty());
        assert_eq!(pool.pick(&mut Rng::new(1)), None);
    }

    #[test]
    fn zero_weight_is_dropped() {
        let pool = Pool::new([("a", 0u32), ("b", 5)]);
        assert_eq!(all_ids(&pool), vec!["b"]);
        assert_eq!(pool.total_weight(), 5);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn duplicate_id_is_dropped() {
        let pool = Pool::new([("a", 3u32), ("a", 7)]);
        assert_eq!(all_ids(&pool), vec!["a"]);
        assert_eq!(pool.total_weight(), 3, "重复项不能把概率翻倍");
    }

    #[test]
    fn retained_filters_and_recounts() {
        let pool = Pool::new([("stretch", 5u32), ("mirror", 5), ("move_run", 10)]);
        let pool = pool.retained(&["stretch", "mirror"]);
        assert_eq!(all_ids(&pool), vec!["stretch", "mirror"]);
        assert_eq!(
            pool.total_weight(),
            10,
            "过滤后权重要重算，否则概率会被摊薄"
        );

        let empty = pool.retained(&[]);
        assert!(empty.is_empty());
        assert_eq!(empty.pick(&mut Rng::new(1)), None);
    }

    /// 权重 1:3 应该给出 25% : 75%，而不是各一半。
    #[test]
    fn weights_decide_the_ratio() {
        let pool = Pool::new([("light", 1u32), ("heavy", 3)]);
        let mut rng = Rng::new(4242);
        let mut light = 0u32;
        const N: u32 = 10_000;
        for _ in 0..N {
            match pool.pick(&mut rng) {
                Some("light") => light += 1,
                Some("heavy") => {}
                other => panic!("抽出了池外的东西：{other:?}"),
            }
        }
        assert!(
            (2300..2700).contains(&light),
            "权重 1:3 应该约 25%，实测 {light}/{N}"
        );
    }

    /// 只抽池子里的东西，抽多少次都一样。
    #[test]
    fn pick_stays_inside_the_pool() {
        let pool = Pool::new([("stretch", 20u32), ("mirror", 20)]);
        let mut rng = Rng::new(7);
        for _ in 0..10_000 {
            let id = pool.pick(&mut rng).expect("非空池必须抽得出");
            assert!(all_ids(&pool).contains(&id));
        }
    }
}
