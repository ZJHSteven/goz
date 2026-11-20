//! 硬链接核对失败后的有限内存重试队列。
//!
//! NTFS 的 HARD_LINK_CHANGE 只告诉我们「某个文件的链接发生了变化」，
//! 却不携带全部当前链接。调用 Win32 枚举可能碰上文件正在删除、移动、
//! 被另一进程临时锁住，或刚创建的父目录尚未被本批 USN 更新解析。
//!
//! 单次失败不能永久跳过，更不能把历史累计失败次数当成当前坏文件数。
//! 本模块只决定「何时重试」，不触碰磁盘或索引，故能用纯单元测试验证。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use goz_core::types::Frn;

/// 每轮最多重新访问这么多文件，避免一个繁忙目录拖慢正常 USN 跟踪。
pub(crate) const MAX_RETRIES_PER_TICK: usize = 32;
/// 队列有硬上限：若越过上限，调用者必须重建或报告不可用，不能丢掉 FRN。
pub(crate) const MAX_PENDING: usize = 8192;
/// 从首次失败起，指数退避；上限确保临时性问题不会一直不再访问。
const BASE_DELAY: Duration = Duration::from_millis(250);
const MAX_DELAY: Duration = Duration::from_secs(60);
/// 日志不能随高并发 USN 记录每50ms刷屏，只汇总低频告警。
const WARN_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
struct Retry {
    failures: u32,
    first_seen: Instant,
    next_at: Instant,
}

/// 只由每个卷唯一的 USN tail 线程拥有，不需要另外加互斥锁。
#[derive(Default)]
pub(crate) struct LinkRetryQueue {
    waiting: BTreeMap<Frn, Retry>,
    last_warning: Option<Instant>,
}

impl LinkRetryQueue {
    pub(crate) fn len(&self) -> usize {
        self.waiting.len()
    }

    /// 在进行第一次 Win32 枚举前，就把这些 FRN 声明为待核对。
    /// 此处去重，避免累积 reason bit 引起同批文件被重复枚举。
    pub(crate) fn mark_dirty(&mut self, frns: &[Frn], now: Instant) -> Result<Vec<Frn>, ()> {
        let unique: std::collections::BTreeSet<Frn> = frns.iter().copied().collect();
        let new_count = unique
            .iter()
            .filter(|frn| !self.waiting.contains_key(frn))
            .count();
        if self.len() + new_count > MAX_PENDING {
            // 不能留下「被放弃但未报告」的文件。
            return Err(());
        }
        for &frn in &unique {
            self.waiting.entry(frn).or_insert(Retry {
                failures: 0,
                first_seen: now,
                next_at: now,
            });
        }
        Ok(unique.into_iter().collect())
    }

    /// 只有确认全部链接已经成功核对（或收到权威的 FILE_DELETE）
    /// 才能撤销一个待办项。失败计数是历史指标，不在这里修改。
    pub(crate) fn resolved(&mut self, frn: Frn) {
        self.waiting.remove(&frn);
    }

    pub(crate) fn failed(&mut self, frn: Frn, now: Instant) {
        let Some(pending) = self.waiting.get_mut(&frn) else {
            return;
        };
        pending.failures = pending.failures.saturating_add(1);
        let shift = pending.failures.saturating_sub(1).min(8);
        let delay = (BASE_DELAY * (1 << shift)).min(MAX_DELAY);
        pending.next_at = now + delay;
    }

    /// 返回即将进行的一小批重试；这里不删队列，避免失败后丢失记录。
    pub(crate) fn due(&self, now: Instant) -> Vec<Frn> {
        self.waiting
            .iter()
            .filter(|(_, retry)| retry.next_at <= now)
            .take(MAX_RETRIES_PER_TICK)
            .map(|(frn, _)| *frn)
            .collect()
    }

    /// 长时间无法修复的记录应触发受控全量重建，而不是无限提示成功。
    pub(crate) fn oldest_age(&self, now: Instant) -> Option<Duration> {
        self.waiting
            .values()
            .map(|item| now.saturating_duration_since(item.first_seen))
            .max()
    }

    pub(crate) fn clear(&mut self) {
        self.waiting.clear();
    }

    /// 仅首次发现新失败时输出一条聚合警告，错误码样本来自实际 Win32 返回。
    pub(crate) fn should_warn(&mut self, now: Instant) -> bool {
        if self
            .last_warning
            .is_some_and(|last| now.saturating_duration_since(last) < WARN_COOLDOWN)
        {
            return false;
        }
        self.last_warning = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deduplicates_same_frn_in_one_usn_batch() {
        let mut q = LinkRetryQueue::default();
        let t = Instant::now();
        assert_eq!(
            q.mark_dirty(&[Frn(7), Frn(7), Frn(8)], t),
            Ok(vec![Frn(7), Frn(8)])
        );
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn a_transient_error_retries_then_recovers() {
        let mut q = LinkRetryQueue::default();
        let t = Instant::now();
        q.mark_dirty(&[Frn(42)], t).unwrap();
        q.failed(Frn(42), t);
        assert!(q.due(t).is_empty());
        assert_eq!(q.due(t + Duration::from_millis(251)), vec![Frn(42)]);
        q.resolved(Frn(42));
        assert!(q.due(t + Duration::from_secs(100)).is_empty());
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn retry_backoff_is_bounded() {
        let mut q = LinkRetryQueue::default();
        let t = Instant::now();
        q.mark_dirty(&[Frn(6)], t).unwrap();
        for i in 0..20 {
            q.failed(Frn(6), t + Duration::from_secs(i));
        }
        assert!(q.due(t + Duration::from_secs(20)).is_empty());
        assert!(q.due(t + Duration::from_secs(80)).contains(&Frn(6)));
    }

    #[test]
    fn bounded_queue_refuses_to_silently_drop_records() {
        let mut q = LinkRetryQueue::default();
        let t = Instant::now();
        let frns: Vec<Frn> = (0..MAX_PENDING as u64).map(Frn).collect();
        q.mark_dirty(&frns, t).unwrap();
        assert!(q.mark_dirty(&[Frn(MAX_PENDING as u64)], t).is_err());
        assert_eq!(q.len(), MAX_PENDING);
    }

    #[test]
    fn the_age_is_from_first_failure_not_last_retry() {
        let mut q = LinkRetryQueue::default();
        let t = Instant::now();
        q.mark_dirty(&[Frn(9)], t).unwrap();
        q.failed(Frn(9), t + Duration::from_secs(60));
        assert_eq!(
            q.oldest_age(t + Duration::from_secs(120)),
            Some(Duration::from_secs(120))
        );
        q.clear();
        assert_eq!(q.oldest_age(t + Duration::from_secs(200)), None);
    }

    #[test]
    fn high_volume_failures_do_not_flood_the_service_log() {
        let mut q = LinkRetryQueue::default();
        let t = Instant::now();
        assert!(q.should_warn(t));
        assert!(!q.should_warn(t + Duration::from_secs(20)));
        assert!(q.should_warn(t + Duration::from_secs(61)));
    }
}
