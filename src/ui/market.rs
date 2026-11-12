//! 盘口（深度）与逐笔成交列表
//!
//! 锁纪律：md 是全系统最热的锁（depth20@100ms / aggTrade 每条 WS 消息都要写），
//! 这里统一"短锁拷贝 → 释放 → 绘制"，绝不在面板构建期间持有锁。
//!
//! 里程碑 1：常量、页签枚举与盘口动画纯状态（BookAnim）原样迁移；
//! 面板渲染在里程碑 5 移植（GPUI flex 行 + 深度条 quad）。

use crate::ui::theme;
use rust_decimal::Decimal;

/// 每侧显示档数下限（面板过矮时的保底）。按用户要求每侧再增一档
pub const BOOK_LEVELS: usize = 12;
/// 每侧显示档数上限：与订阅的部分深度流档数一致（@depth20 推满 20 档）。
/// 上限再大也没有数据——take(n) 只是取到原始推送为止，面板反而出现
/// 半幅留白；档位数随面板高度在此区间自适应，行高微调（±<1pt）填满面板
pub const BOOK_MAX_LEVELS: usize = 20;

/// 盘口行闪光/残影强度：取全局 FLASH_ALPHA 的一半——盘口是整列大量行同时
/// 更新，0.28 的整行底色在高频推送下连成整片红/绿，压到一半后只剩
/// "这一格刚变化"的提示意味，不再抢占视线。深度条颜色不走此值
pub const BOOK_FLASH_ALPHA: f32 = theme::FLASH_ALPHA * 0.5;

/// 右栏内容页签：盘口 / 成交明细
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookTab {
    /// 盘口（档位随面板高度自适应，12~20 档，行高微调填满面板）
    Book,
    /// 成交明细
    Trades,
}

/// 「档位」胶囊条的固定占位高度（含上方分隔线）：盘口的自适应档数须按此
/// 预算可用高度（原 egui 版语义，GPUI 版渲染沿用）
pub const BOOK_GROUP_H: f32 = 36.0;

/// 已消失盘口档位的残影
#[derive(Debug, Clone, Copy)]
pub struct BookGhost {
    /// true = 卖盘，false = 买盘
    pub is_ask: bool,
    /// 显示行槽位（0 = 该侧块顶部行）
    pub slot: usize,
    /// 消失时的深度条占比
    pub ratio: f64,
    /// 消失时刻（单调时钟秒域；GPUI 版由调用方传 Instant 基准的秒数）
    pub t0: f64,
}

/// 盘口动画状态：上一帧价格集合 + 消失档位残影（跨帧存于 BnApp，缓冲区复用零堆分配）
#[derive(Debug, Clone, Default)]
pub struct BookAnim {
    prev_ask: Vec<(Decimal, f64)>,
    prev_bid: Vec<(Decimal, f64)>,
    ghosts: Vec<BookGhost>,
}

impl BookAnim {
    /// 清空（切换交易对/市场时调用，避免跨符号误判）
    pub fn clear(&mut self) {
        self.prev_ask.clear();
        self.prev_bid.clear();
        self.ghosts.clear();
    }

    /// 当前未过期的残影列表（渲染层绘制用）
    pub fn ghosts(&self) -> &[BookGhost] {
        &self.ghosts
    }

    /// 与上一帧 diff：消失的档位记为残影，过期残影清除。
    /// `now` 取单调时钟秒（`Instant::now()` 相对某基准的秒数），同一时间域
    /// 内与 GHOST_SECS 比较即可；egui 版取内部秒表，语义一致
    pub fn update(
        &mut self,
        now: f64,
        is_ask: bool,
        rows: &[(Decimal, Decimal)],
        cums: &[f64],
        max_cum: f64,
    ) {
        let mut prev = std::mem::take(if is_ask {
            &mut self.prev_ask
        } else {
            &mut self.prev_bid
        });
        self.ghosts.retain(|g| now - g.t0 < theme::GHOST_SECS);
        for (p, ratio) in &prev {
            // 仍在当前帧的档位不是残影
            if rows.iter().any(|(cp, _)| cp == p) {
                continue;
            }
            // 消失档位在当前升序列中的排序位 → 显示槽位
            let idx = rows.partition_point(|(cp, _)| *cp < *p);
            let slot = if is_ask {
                // ask 显示倒序：slot = len-1-idx；idx==len（远端消失）时无槽位
                rows.len().checked_sub(1 + idx)
            } else {
                // bid 显示正序；idx==len（远端消失）时无槽位
                (idx < rows.len()).then_some(idx)
            };
            let Some(slot) = slot else { continue };
            if self.ghosts.len() >= 8 {
                self.ghosts.remove(0);
            }
            self.ghosts.push(BookGhost {
                is_ask,
                slot,
                ratio: *ratio,
                t0: now,
            });
        }
        // 记录本帧（价格, 深度占比），供下一帧 diff；复用缓冲区
        prev.clear();
        for (i, (p, _)) in rows.iter().enumerate() {
            prev.push((*p, cums[i] / max_cum));
        }
        if is_ask {
            self.prev_ask = prev;
        } else {
            self.prev_bid = prev;
        }
    }
}
