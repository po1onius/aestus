import { CalendarRange, Loader2 } from "lucide-react";
import { type ReactNode, useMemo } from "react";
import { DatePickerInput } from "../components/DatePickerInput";
import { EChart, type EChartOption } from "../components/EChart";
import { Metric } from "../components/Metric";
import { SlidingTabList } from "../components/SlidingTabList";
import { daysAgoInputValue, formatTokenCount, todayInputValue } from "../lib/format";
import {
  cx,
  panelClass,
  spinnerClass,
  tabClass,
  tabContentClass,
  tabIdleClass,
  tabSelectedClass,
} from "../lib/ui";
import type {
  DashboardTheme,
  UsageApiKeyPoint,
  UsageModelPoint,
  UsagePointCount,
  UsageResponse,
  UsageTimelineResponse,
  UsageUserPoint,
} from "../types";

interface UsagePageProps {
  theme: DashboardTheme;
  usage: UsageResponse | null;
  timeline: UsageTimelineResponse | null;
  loading: boolean;
  timelineLoading: boolean;
  startDate: string;
  endDate: string;
  pointCount: UsagePointCount;
  onStartDateChange: (value: string) => void;
  onEndDateChange: (value: string) => void;
  onPointCountChange: (value: UsagePointCount) => void;
  onRangeChange: (startDate: string, endDate: string) => void;
}

const chartNumberFormatter = new Intl.NumberFormat("zh-CN", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const CHART_FONT_FAMILY = "'IBM Plex Sans Variable', 'Noto Sans SC Variable', sans-serif";

/** 三张模型图共用同一套颜色，确保同一统计结果中的模型视觉语义保持一致。 */
const MODEL_COLORS = [
  "#5b5bd6",
  "#3977d4",
  "#2e8b8b",
  "#9b6ac4",
  "#c47a3a",
  "#557a95",
  "#a45f78",
  "#7b8492",
];
const CONSUMER_COLORS = [
  "#4667c8",
  "#7b61c8",
  "#31839a",
  "#a45e91",
  "#6d8b57",
  "#c17149",
  "#4d799e",
  "#8a7564",
];
const OTHER_MODEL_PROVIDER = "all";
const OTHER_MODEL_NAME = "其他模型";
const OTHER_API_KEY_NAME = "其他 API Key";
const OTHER_USERNAME = "其他用户";

export function UsagePage({
  theme,
  usage,
  timeline,
  loading,
  timelineLoading,
  startDate,
  endDate,
  pointCount,
  onStartDateChange,
  onEndDateChange,
  onPointCountChange,
  onRangeChange,
}: UsagePageProps) {
  const darkMode = theme === "dark";
  const modelPoints = useMemo(() => compactModelPoints(usage?.models ?? []), [usage]);
  const apiKeyPoints = useMemo(() => compactApiKeyPoints(usage?.api_keys ?? []), [usage]);
  const userPoints = useMemo(() => compactUserPoints(usage?.users ?? []), [usage]);
  const usageScope = usage?.scope ?? "current_user";
  const allUsers = usageScope === "all_users";
  const activeTimeline =
    usage &&
    timeline &&
    usage.start_at === timeline.start_at &&
    usage.end_at === timeline.end_at &&
    usage.scope === timeline.scope &&
    timeline.point_count === pointCount
      ? timeline
      : null;
  const timelineOption = useMemo(
    () => buildTimelineOption(activeTimeline, modelPoints, darkMode),
    [activeTimeline, darkMode, modelPoints],
  );
  const modelShareOption = useMemo(
    () => buildModelShareOption(modelPoints, darkMode),
    [darkMode, modelPoints],
  );
  const consumerShareOption = useMemo(
    () =>
      allUsers
        ? buildConsumerShareOption(
            userPoints.map((point) => ({
              name: point.username,
              total_tokens: point.total_tokens,
            })),
            "用户 Token",
            darkMode,
          )
        : buildConsumerShareOption(apiKeyPoints, "API Key Token", darkMode),
    [allUsers, apiKeyPoints, darkMode, userPoints],
  );
  const modelRankOption = useMemo(
    () => buildModelRankOption(modelPoints, darkMode),
    [darkMode, modelPoints],
  );
  const hasUsage = usage !== null && parseTokenCount(usage.period.total_tokens) > 0n;
  const today = todayInputValue();
  const presetRanges = [
    { startDate: today, endDate: today },
    { startDate: daysAgoInputValue(6), endDate: today },
    { startDate: daysAgoInputValue(29), endDate: today },
  ];
  const selectedPresetIndex = presetRanges.findIndex(
    (range) => range.startDate === startDate && range.endDate === endDate,
  );

  return (
    <section className="grid gap-4">
      <section className="grid grid-cols-2 gap-3 xl:grid-cols-6" aria-label="用量数据概览">
        <div className={`${panelClass} col-span-2 grid gap-4 p-4 xl:col-span-2`}>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2 text-sm font-semibold text-slate-800 dark:text-slate-200">
              <CalendarRange className="text-slate-500 dark:text-slate-400" size={18} />
              <strong>统计时间</strong>
            </div>
            <SlidingTabList
              count={3}
              selectedIndex={selectedPresetIndex}
              ariaLabel="快捷时间范围"
              role="group"
            >
              <RangeButton
                label="今天"
                startDate={presetRanges[0].startDate}
                endDate={presetRanges[0].endDate}
                currentStartDate={startDate}
                currentEndDate={endDate}
                onRangeChange={onRangeChange}
              />
              <RangeButton
                label="近 7 天"
                startDate={presetRanges[1].startDate}
                endDate={presetRanges[1].endDate}
                currentStartDate={startDate}
                currentEndDate={endDate}
                onRangeChange={onRangeChange}
              />
              <RangeButton
                label="近 30 天"
                startDate={presetRanges[2].startDate}
                endDate={presetRanges[2].endDate}
                currentStartDate={startDate}
                currentEndDate={endDate}
                onRangeChange={onRangeChange}
              />
            </SlidingTabList>
          </div>
          <div className="grid gap-2 sm:grid-cols-2">
            <div className="grid gap-1.5">
              <span className="text-xs font-medium text-slate-500 dark:text-slate-400">开始</span>
              <DatePickerInput
                value={startDate}
                max={endDate}
                disabled={loading}
                ariaLabel="选择开始日期"
                onChange={onStartDateChange}
              />
            </div>
            <div className="grid gap-1.5">
              <span className="text-xs font-medium text-slate-500 dark:text-slate-400">结束</span>
              <DatePickerInput
                value={endDate}
                min={startDate}
                max={today}
                disabled={loading}
                ariaLabel="选择结束日期"
                onChange={onEndDateChange}
              />
            </div>
          </div>
        </div>

        {usage && (
          <>
            <Metric
              label={allUsers ? "全体剩余 Token" : "剩余 Token"}
              value={formatTokenCount(usage.remaining_tokens)}
              tone="good"
              title={usage.remaining_tokens}
            />
            <Metric
              label={allUsers ? "全体累计消耗" : "累计消耗"}
              value={formatTokenCount(usage.consumed_tokens)}
              title={usage.consumed_tokens}
            />
            <Metric
              label={allUsers ? "全体时段消耗" : "所选时段消耗"}
              value={formatTokenCount(usage.period.total_tokens)}
              title={usage.period.total_tokens}
            />
            <Metric
              label={allUsers ? "全体请求次数" : "请求次数"}
              value={formatTokenCount(usage.period.request_count)}
              title={usage.period.request_count}
            />
          </>
        )}
      </section>

      {loading && !usage ? (
        <div className={`${panelClass} flex min-h-64 items-center justify-center gap-3 p-8 text-sm text-slate-500 dark:text-slate-400`}>
          <Loader2 className={spinnerClass} size={24} />
          <span>正在加载用量统计</span>
        </div>
      ) : usage ? (
        <div className="grid gap-4 xl:grid-cols-2" aria-busy={loading || timelineLoading}>
          <article className={`${panelClass} grid gap-3 p-4 xl:col-span-2`}>
            <ChartHeading
              title="Token 使用趋势"
              controls={
                <TimelineDensityControl
                  value={pointCount}
                  onChange={onPointCountChange}
                />
              }
            />
            {!activeTimeline && (timelineLoading || loading) ? (
              <ChartLoadingState />
            ) : hasUsage && activeTimeline ? (
              <EChart option={timelineOption} ariaLabel="各模型 Token 使用趋势图" />
            ) : (
              <ChartEmptyState />
            )}
          </article>
          <article className={`${panelClass} grid gap-3 p-4`}>
            <ChartHeading title="消耗占比" />
            <div className="grid gap-4 sm:grid-cols-2">
              <section className="min-w-0">
                <h3 className="mb-2 text-center text-xs font-semibold text-slate-500 dark:text-slate-400">模型</h3>
                {modelPoints.length > 0 ? (
                  <EChart
                    option={modelShareOption}
                    ariaLabel="模型 Token 消耗占比环形图"
                    className="h-72 min-h-64 w-full"
                  />
                ) : (
                  <ShareChartEmptyState />
                )}
              </section>
              <section className="min-w-0">
                <h3 className="mb-2 text-center text-xs font-semibold text-slate-500 dark:text-slate-400">{allUsers ? "用户" : "API Key"}</h3>
                {(allUsers ? userPoints.length : apiKeyPoints.length) > 0 ? (
                  <EChart
                    option={consumerShareOption}
                    ariaLabel={
                      allUsers
                        ? "用户 Token 使用占比环形图"
                        : "API Key Token 使用占比环形图"
                    }
                    className="h-72 min-h-64 w-full"
                  />
                ) : (
                  <ShareChartEmptyState />
                )}
              </section>
            </div>
          </article>
          <article className={`${panelClass} grid gap-3 p-4`}>
            <ChartHeading title="模型消耗排行" />
            {modelPoints.length > 0 ? (
              <EChart option={modelRankOption} ariaLabel="模型 Token 消耗排行柱状图" />
            ) : (
              <ChartEmptyState />
            )}
          </article>
        </div>
      ) : null}
    </section>
  );
}

interface RangeButtonProps {
  label: string;
  startDate: string;
  endDate: string;
  currentStartDate: string;
  currentEndDate: string;
  onRangeChange: (startDate: string, endDate: string) => void;
}

function RangeButton(props: RangeButtonProps) {
  const selected =
    props.startDate === props.currentStartDate && props.endDate === props.currentEndDate;
  return (
    <button
      type="button"
      className={cx(tabClass, selected ? tabSelectedClass : tabIdleClass)}
      aria-pressed={selected}
      onClick={() => props.onRangeChange(props.startDate, props.endDate)}
    >
      <span className={tabContentClass}>{props.label}</span>
    </button>
  );
}

function TimelineDensityControl({
  value,
  onChange,
}: {
  value: UsagePointCount;
  onChange: (value: UsagePointCount) => void;
}) {
  return (
    <SlidingTabList
      count={2}
      selectedIndex={value === 20 ? 0 : 1}
      ariaLabel="趋势图数据密度"
      role="group"
    >
      {([
        { value: 20, label: "疏" },
        { value: 50, label: "密" },
      ] as const).map((option) => (
        <button
          key={option.value}
          type="button"
          className={cx(
            tabClass,
            "min-w-9",
            value === option.value ? tabSelectedClass : tabIdleClass,
          )}
          aria-pressed={value === option.value}
          aria-label={`${option.label}，${option.value} 个数据点`}
          onClick={() => onChange(option.value)}
        >
          <span className={tabContentClass}>{option.label}</span>
        </button>
      ))}
    </SlidingTabList>
  );
}

function ChartHeading({
  title,
  controls,
}: {
  title: string;
  controls?: ReactNode;
}) {
  return (
    <div className="flex min-h-8 flex-wrap items-center justify-between gap-3">
      <h2 className="text-sm font-semibold text-slate-900 dark:text-slate-100">{title}</h2>
      {controls && (
        <div className="flex items-center gap-2 text-slate-500 dark:text-slate-400">
          {controls}
        </div>
      )}
    </div>
  );
}

function ChartLoadingState() {
  return (
    <div className="flex min-h-72 items-center justify-center gap-2 text-sm text-slate-500 dark:text-slate-400">
      <Loader2 className={spinnerClass} size={22} />
      <span>正在加载使用趋势</span>
    </div>
  );
}

function ChartEmptyState() {
  return <div className="flex min-h-72 items-center justify-center px-5 text-center text-sm text-slate-500 dark:text-slate-400">所选时间段还没有 Token 用量</div>;
}

function ShareChartEmptyState() {
  return <div className="flex min-h-64 items-center justify-center text-sm text-slate-500 dark:text-slate-400">暂无用量</div>;
}

/** Canvas 图表无法直接读取 Tailwind 的文字与边框色，因此由当前主题显式生成同源色板。 */
function chartThemeColors(darkMode: boolean) {
  return darkMode
    ? {
        text: "#d0d5dd",
        mutedText: "#98a2b3",
        axis: "#475467",
        grid: "#28313e",
        pieBorder: "#0c0f14",
      }
    : {
        text: "#344054",
        mutedText: "#667085",
        axis: "#d0d5dd",
        grid: "#e4e7ec",
        pieBorder: "#ffffff",
      };
}

function chartTooltip(darkMode: boolean) {
  return darkMode
    ? {
        backgroundColor: "rgba(19, 24, 32, 0.96)",
        borderColor: "#28313e",
        textStyle: { color: "#f2f4f7" },
      }
    : {
        backgroundColor: "rgba(255, 255, 255, 0.96)",
        borderColor: "#e4e7ec",
        textStyle: { color: "#344054" },
      };
}

function buildTimelineOption(
  usage: UsageTimelineResponse | null,
  models: UsageModelPoint[],
  darkMode: boolean,
): EChartOption {
  const colors = chartThemeColors(darkMode);
  const buckets = usage?.timeline ?? [];
  const visibleModelKeys = new Set(
    models
      .filter((point) => point.provider !== OTHER_MODEL_PROVIDER)
      .map((point) => modelKey(point)),
  );
  const otherModel = models.find((point) => point.provider === OTHER_MODEL_PROVIDER);
  const otherModelKey = otherModel ? modelKey(otherModel) : null;
  const tokensByBucket = buckets.map((bucket) => {
    const totals = new Map<string, bigint>();

    // 后端始终返回完整的 20/50 个时间桶，桶内只携带实际存在的模型分组。这里按整个
    // 统计区间的模型排行逐桶合并低频模型，保证三张模型图的系列和颜色完全一致。
    for (const point of bucket.models) {
      const originalModelKey = modelKey(point);
      const targetModelKey = visibleModelKeys.has(originalModelKey)
        ? originalModelKey
        : otherModelKey;
      if (!targetModelKey) {
        continue;
      }
      totals.set(
        targetModelKey,
        (totals.get(targetModelKey) ?? 0n) + parseTokenCount(point.total_tokens),
      );
    }
    return totals;
  });
  const selectedDurationMs = usage
    ? Math.max(0, Date.parse(usage.end_at) - Date.parse(usage.start_at))
    : 0;

  return {
    aria: { enabled: true },
    animationDuration: 350,
    textStyle: { fontFamily: CHART_FONT_FAMILY },
    color: MODEL_COLORS,
    grid: { top: 42, right: 20, bottom: 36, left: 70 },
    legend: {
      type: "scroll",
      top: 0,
      right: 12,
      left: 12,
      data: models.map(modelLabel),
      textStyle: { color: colors.text },
    },
    tooltip: {
      ...chartTooltip(darkMode),
      trigger: "axis",
      valueFormatter: (value: unknown) => formatChartValue(value),
    },
    xAxis: {
      type: "category",
      // category 的两侧留出半个桶宽，数据点位于桶中心；轴的左右边缘分别对应用户选择
      // 的 start_at/end_at，而不是第一条和最后一条请求的发生时间。
      boundaryGap: true,
      data: buckets.map((bucket) => formatBucketRange(bucket.started_at, bucket.ended_at)),
      axisLabel: {
        color: colors.mutedText,
        hideOverlap: true,
        formatter: (_value: string, index: number) =>
          formatBucketLabel(buckets[index]?.started_at ?? "", selectedDurationMs),
      },
      axisLine: { lineStyle: { color: colors.axis } },
    },
    yAxis: {
      type: "value",
      minInterval: 1,
      axisLabel: { color: colors.mutedText, formatter: (value: number) => chartNumberFormatter.format(value) },
      splitLine: { lineStyle: { color: colors.grid } },
    },
    series: models.map((model) => {
      const key = modelKey(model);
      return {
        name: modelLabel(model),
        type: "line",
        stack: "models",
        smooth: true,
        showSymbol: false,
        areaStyle: { opacity: 0.18 },
        emphasis: { focus: "series" },
        data: tokensByBucket.map((bucket) =>
          chartValue((bucket.get(key) ?? 0n).toString()),
        ),
      };
    }),
  };
}

function buildModelShareOption(points: UsageModelPoint[], darkMode: boolean): EChartOption {
  const colors = chartThemeColors(darkMode);
  return {
    aria: { enabled: true },
    animationDuration: 350,
    textStyle: { fontFamily: CHART_FONT_FAMILY },
    color: MODEL_COLORS,
    tooltip: {
      ...chartTooltip(darkMode),
      trigger: "item",
      valueFormatter: (value: unknown) => formatChartValue(value),
    },
    legend: {
      type: "scroll",
      bottom: 0,
      textStyle: { color: colors.text },
    },
    series: [
      {
        name: "模型 Token",
        type: "pie",
        radius: ["45%", "70%"],
        center: ["50%", "43%"],
        avoidLabelOverlap: true,
        itemStyle: { borderColor: colors.pieBorder, borderWidth: 2 },
        label: { formatter: "{d}%", color: colors.text },
        data: points.map((point) => ({
          name: modelLabel(point),
          value: chartValue(point.total_tokens),
        })),
      },
    ],
  };
}

function buildConsumerShareOption(
  points: Array<{ name: string; total_tokens: string }>,
  seriesName: string,
  darkMode: boolean,
): EChartOption {
  const colors = chartThemeColors(darkMode);
  return {
    aria: { enabled: true },
    animationDuration: 350,
    textStyle: { fontFamily: CHART_FONT_FAMILY },
    color: CONSUMER_COLORS,
    tooltip: {
      ...chartTooltip(darkMode),
      trigger: "item",
      valueFormatter: (value: unknown) => formatChartValue(value),
    },
    legend: {
      type: "scroll",
      bottom: 0,
      textStyle: { color: colors.text },
    },
    series: [
      {
        name: seriesName,
        type: "pie",
        radius: ["45%", "70%"],
        center: ["50%", "43%"],
        avoidLabelOverlap: true,
        itemStyle: { borderColor: colors.pieBorder, borderWidth: 2 },
        label: { formatter: "{d}%", color: colors.text },
        data: points.map((point) => ({
          name: point.name,
          value: chartValue(point.total_tokens),
        })),
      },
    ],
  };
}

function buildModelRankOption(points: UsageModelPoint[], darkMode: boolean): EChartOption {
  const colors = chartThemeColors(darkMode);
  // 排行图只有一个 bar series，必须给每个数据项显式分配颜色，才能与占比图及趋势图
  // 按模型一一对应；只设置 series color 会让全部柱子使用同一种颜色。
  const ordered = points
    .map((point, index) => ({ point, color: MODEL_COLORS[index % MODEL_COLORS.length] }))
    .reverse();
  return {
    aria: { enabled: true },
    animationDuration: 350,
    textStyle: { fontFamily: CHART_FONT_FAMILY },
    grid: { top: 12, right: 28, bottom: 32, left: 126 },
    tooltip: {
      ...chartTooltip(darkMode),
      trigger: "axis",
      axisPointer: { type: "shadow" },
      valueFormatter: (value: unknown) => formatChartValue(value),
    },
    xAxis: {
      type: "value",
      minInterval: 1,
      axisLabel: { color: colors.mutedText, formatter: (value: number) => chartNumberFormatter.format(value) },
      splitLine: { lineStyle: { color: colors.grid } },
    },
    yAxis: {
      type: "category",
      data: ordered.map(({ point }) => modelLabel(point)),
      axisLabel: { color: colors.text, width: 108, overflow: "truncate" },
      axisLine: { lineStyle: { color: colors.axis } },
      axisTick: { show: false },
    },
    series: [
      {
        name: "Token",
        type: "bar",
        barMaxWidth: 22,
        itemStyle: { borderRadius: [0, 4, 4, 0] },
        data: ordered.map(({ point, color }) => ({
          value: chartValue(point.total_tokens),
          itemStyle: { color },
        })),
      },
    ],
  };
}

/** 模型过多时保留前七项，其余用 BigInt 精确汇总为“其他”。 */
function compactModelPoints(points: UsageModelPoint[]) {
  if (points.length <= 8) {
    return points;
  }

  const visible = points.slice(0, 7);
  const remaining = points.slice(7);
  const otherTokens = remaining.reduce(
    (total, point) => total + parseTokenCount(point.total_tokens),
    0n,
  );
  const otherRequests = remaining.reduce(
    (total, point) => total + parseTokenCount(point.request_count),
    0n,
  );
  const otherPercentage = remaining.reduce((total, point) => total + point.percentage, 0);
  return [
    ...visible,
    {
      provider: OTHER_MODEL_PROVIDER,
      model: OTHER_MODEL_NAME,
      total_tokens: otherTokens.toString(),
      request_count: otherRequests.toString(),
      percentage: otherPercentage,
    },
  ];
}

/** API Key 过多时同样保留前七项，避免名称图例挤压饼图主体。 */
function compactApiKeyPoints(points: UsageApiKeyPoint[]) {
  if (points.length <= 8) {
    return points;
  }

  const visible = points.slice(0, 7);
  const remaining = points.slice(7);
  const otherTokens = remaining.reduce(
    (total, point) => total + parseTokenCount(point.total_tokens),
    0n,
  );
  const otherRequests = remaining.reduce(
    (total, point) => total + parseTokenCount(point.request_count),
    0n,
  );
  const otherPercentage = remaining.reduce((total, point) => total + point.percentage, 0);
  return [
    ...visible,
    {
      name: OTHER_API_KEY_NAME,
      total_tokens: otherTokens.toString(),
      request_count: otherRequests.toString(),
      percentage: otherPercentage,
    },
  ];
}

/** admin 用户分布与普通用户 API Key 分布使用相同的前七项展示规则。 */
function compactUserPoints(points: UsageUserPoint[]) {
  if (points.length <= 8) {
    return points;
  }

  const visible = points.slice(0, 7);
  const remaining = points.slice(7);
  const otherTokens = remaining.reduce(
    (total, point) => total + parseTokenCount(point.total_tokens),
    0n,
  );
  const otherRequests = remaining.reduce(
    (total, point) => total + parseTokenCount(point.request_count),
    0n,
  );
  const otherPercentage = remaining.reduce((total, point) => total + point.percentage, 0);
  return [
    ...visible,
    {
      user_id: "",
      username: OTHER_USERNAME,
      total_tokens: otherTokens.toString(),
      request_count: otherRequests.toString(),
      percentage: otherPercentage,
    },
  ];
}

function modelLabel(point: UsageModelPoint) {
  return point.provider === OTHER_MODEL_PROVIDER
    ? point.model
    : `${point.model} · ${point.provider}`;
}

function modelKey(point: { provider: string; model: string }) {
  return JSON.stringify([point.provider, point.model]);
}

function parseTokenCount(value: string) {
  try {
    return BigInt(value);
  } catch {
    return 0n;
  }
}

function chartValue(value: string) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.max(0, parsed) : 0;
}

function formatChartValue(value: unknown) {
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isFinite(parsed) ? new Intl.NumberFormat("zh-CN").format(parsed) : "0";
}

function formatBucketRange(startedAt: string, endedAt: string) {
  return `${formatBucketBoundary(startedAt)} – ${formatBucketBoundary(endedAt)}`;
}

function formatBucketBoundary(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    ...(date.getSeconds() !== 0 ? { second: "2-digit" } : {}),
  }).format(date);
}

function formatBucketLabel(value: string, selectedDurationMs: number) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat("zh-CN", {
    ...(selectedDurationMs > 86_400_000 ? { month: "2-digit", day: "2-digit" } : {}),
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}
