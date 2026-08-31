//! Dashboard 日期边界的单一实现。
//!
//! 请求日志与用量统计必须共用服务固定时区；否则同一条请求可能在明细页和
//! 聚合页被归入不同日期。本模块只负责日历边界计算，不为无效时区做运行时降级。

use chrono::{DateTime, Datelike, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

use crate::err::{AppError, AppResult};

pub(super) fn current_service_date(timezone: Tz) -> NaiveDate {
    Utc::now().with_timezone(&timezone).date_naive()
}

/// 将固定服务时区下的自然日起点转为 UTC 时刻。
///
/// 少数 IANA 时区会恰好在午夜向前切换，使 00:00 不存在。逐分钟寻找该日第一个
/// 有效本地时刻，能覆盖整点、半小时及历史时区切换，不引入固定 24 小时假设。
pub(super) fn local_day_start_utc(timezone: Tz, date: NaiveDate) -> AppResult<DateTime<Utc>> {
    for minute_of_day in 0..(24 * 60) {
        let hour = minute_of_day / 60;
        let minute = minute_of_day % 60;
        let local =
            timezone.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0);
        match local {
            LocalResult::Single(value) => return Ok(value.with_timezone(&Utc)),
            // 午夜回拨产生两个同名本地时刻时，取较早的绝对时间作为自然日开端。
            LocalResult::Ambiguous(first, second) => {
                return Ok(first.min(second).with_timezone(&Utc));
            }
            LocalResult::None => {}
        }
    }

    Err(AppError::BadRequest {
        message: format!("时区 {timezone} 中不存在本地日期 {date}"),
    })
}

pub(super) fn local_day_range_utc(
    timezone: Tz,
    date: NaiveDate,
) -> AppResult<(DateTime<Utc>, DateTime<Utc>)> {
    let next_date = date.succ_opt().ok_or_else(|| AppError::BadRequest {
        message: format!("计算日期 {date} 的结束边界时超出支持范围"),
    })?;
    Ok((
        local_day_start_utc(timezone, date)?,
        local_day_start_utc(timezone, next_date)?,
    ))
}
