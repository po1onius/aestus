use serde::{Deserialize, Serialize};

use crate::err::{AppError, AppResult};

const DEFAULT_LIST_PAGE_SIZE: i64 = 100;
const MAX_LIST_PAGE_SIZE: i64 = 200;
// offset 分页足以满足当前管理面板的低频浏览；限制最大偏移，避免任意大 OFFSET 触发
// PostgreSQL 扫描大量无用行。数据量超过该范围时应改为带筛选条件的 keyset API。
const MAX_LIST_OFFSET: i64 = 100_000;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListPageQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ListPageParams {
    limit: i64,
    offset: i64,
}

#[derive(Debug, Serialize)]
pub struct ListPage<T> {
    pub items: Vec<T>,
    pub offset: i64,
    pub limit: i64,
    pub next_offset: Option<i64>,
}

impl ListPageQuery {
    pub fn new(limit: Option<i64>, offset: Option<i64>) -> Self {
        Self { limit, offset }
    }

    pub fn normalize(self) -> AppResult<ListPageParams> {
        let limit = self.limit.unwrap_or(DEFAULT_LIST_PAGE_SIZE);
        let offset = self.offset.unwrap_or(0);
        if !(1..=MAX_LIST_PAGE_SIZE).contains(&limit) {
            return Err(AppError::BadRequest {
                message: format!("limit 必须在 1 到 {MAX_LIST_PAGE_SIZE} 之间"),
            });
        }
        if !(0..=MAX_LIST_OFFSET).contains(&offset) {
            return Err(AppError::BadRequest {
                message: format!("offset 必须在 0 到 {MAX_LIST_OFFSET} 之间"),
            });
        }
        Ok(ListPageParams { limit, offset })
    }
}

impl ListPageParams {
    /// SQL 多读取一条用于判断是否存在下一页，避免额外执行 COUNT 查询。
    pub fn query_limit(self) -> i64 {
        self.limit + 1
    }

    pub fn offset(self) -> i64 {
        self.offset
    }

    pub fn finish<T>(self, mut items: Vec<T>) -> ListPage<T> {
        let has_more = items.len() > self.limit as usize;
        items.truncate(self.limit as usize);
        ListPage {
            items,
            offset: self.offset,
            limit: self.limit,
            next_offset: has_more
                .then_some(self.offset + self.limit)
                .filter(|next_offset| *next_offset <= MAX_LIST_OFFSET),
        }
    }
}
