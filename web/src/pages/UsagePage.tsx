import { Loader2, RotateCcw } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import { EChart, type EChartOption } from "../components/EChart";
import { Metric } from "../components/Metric";
import { formatCompactTokenAmount, formatTokenCount } from "../lib/format";
import { panelClass, spinnerClass } from "../lib/ui";
import type {
  DashboardTheme,
  UsageApiKeyPoint,
  UsageDailyPoint,
  UsageModelPoint,
  UsageResponse,
} from "../types";

interface UsagePageProps {
  theme: DashboardTheme;
  usage: UsageResponse | null;
  loading: boolean;
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
const CONTRIBUTION_PALETTES = {
  light: ["#e5e7eb", "#c6e48b", "#7bc96f", "#239a3b", "#196127"],
  dark: ["#27313d", "#164b35", "#19733f", "#24a148", "#56d364"],
} as const;
const OTHER_MODEL_PROVIDER = "all";
const OTHER_MODEL_NAME = "其他模型";
const OTHER_API_KEY_NAME = "其他 API Key";

export function UsagePage({
  theme,
  usage,
  loading,
}: UsagePageProps) {
  const darkMode = theme === "dark";
  const [contributionViewRevision, setContributionViewRevision] = useState(0);
  const modelPoints = useMemo(() => compactModelPoints(usage?.models ?? []), [usage]);
  const apiKeyPoints = useMemo(() => compactApiKeyPoints(usage?.api_keys ?? []), [usage]);
  // 管理员排行展示真实的前八名用户，不把剩余用户汇总成一个会干扰名次的“其他用户”。
  const userRankPoints = useMemo(() => (usage?.users ?? []).slice(0, 8), [usage]);
  const usageScope = usage?.scope ?? "current_user";
  const allUsers = usageScope === "all_users";
  const contributionOption = useMemo(
    () => buildContributionOption(usage?.daily ?? [], darkMode),
    [darkMode, usage],
  );
  const modelShareOption = useMemo(
    () => buildModelShareOption(modelPoints, darkMode),
    [darkMode, modelPoints],
  );
  const apiKeyShareOption = useMemo(
    () => buildConsumerShareOption(apiKeyPoints, "API Key Token", darkMode),
    [apiKeyPoints, darkMode],
  );
  const rankOption = useMemo(
    () =>
      allUsers
        ? buildRankOption(
            userRankPoints.map((point) => ({
              name: point.username,
              total_tokens: point.total_tokens,
            })),
            CONSUMER_COLORS,
            darkMode,
          )
        : buildRankOption(
            modelPoints.map((point) => ({
              name: modelLabel(point),
              total_tokens: point.total_tokens,
            })),
            MODEL_COLORS,
            darkMode,
          ),
    [allUsers, darkMode, modelPoints, userRankPoints],
  );
  return (
    <section className="grid gap-4">
      <section className="grid grid-cols-2 gap-3 xl:grid-cols-4" aria-label="用量数据概览">
        {usage && (
          <>
            <Metric
              label={allUsers ? "全体剩余 Token" : "剩余 Token"}
              value={formatTokenCount(usage.remaining_tokens)}
              tone="good"
              title={usage.remaining_tokens}
              cornerValue={formatCompactTokenAmount(usage.remaining_tokens)}
            />
            <Metric
              label={allUsers ? "全体累计消耗" : "累计消耗"}
              value={formatTokenCount(usage.consumed_tokens)}
              title={usage.consumed_tokens}
              cornerValue={formatCompactTokenAmount(usage.consumed_tokens)}
            />
            <Metric
              label={allUsers ? "全体历史日志 Token" : "历史日志 Token"}
              value={formatTokenCount(usage.lifetime.total_tokens)}
              title={usage.lifetime.total_tokens}
              cornerValue={formatCompactTokenAmount(usage.lifetime.total_tokens)}
            />
            <Metric
              label={allUsers ? "全体历史请求次数" : "历史请求次数"}
              value={formatTokenCount(usage.lifetime.request_count)}
              title={usage.lifetime.request_count}
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
        <div className="grid gap-4 xl:grid-cols-2" aria-busy={loading}>
          <article className={`${panelClass} grid gap-3 p-4 xl:col-span-2`}>
            <ChartHeading
              title="近一年 Token 活跃度"
              controls={
                <div className="flex flex-wrap items-center justify-end gap-3">
                  <ContributionLegend darkMode={darkMode} />
                  <span className="text-xs text-slate-500 dark:text-slate-400">
                    拖动旋转 · 滚轮缩放 · 高度为对数比例
                  </span>
                  <button
                    type="button"
                    className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-slate-200 px-2.5 text-xs font-medium text-slate-600 transition hover:border-slate-300 hover:bg-slate-50 dark:border-slate-700 dark:text-slate-300 dark:hover:border-slate-600 dark:hover:bg-slate-800"
                    onClick={() => setContributionViewRevision((current) => current + 1)}
                  >
                    <RotateCcw size={14} />
                    重置视角
                  </button>
                </div>
              }
            />
            <EChart
              key={contributionViewRevision}
              option={contributionOption}
              ariaLabel="最近 365 天每日 Token 用量三维贡献图"
              className="h-[34rem] min-h-[30rem] w-full"
            />
          </article>
          <article className={`${panelClass} grid gap-3 p-4`}>
            <ChartHeading title="全历史消耗占比" />
            <div className={allUsers ? "grid gap-4" : "grid gap-4 sm:grid-cols-2"}>
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
              {!allUsers && (
                <section className="min-w-0">
                  <h3 className="mb-2 text-center text-xs font-semibold text-slate-500 dark:text-slate-400">API Key</h3>
                  {apiKeyPoints.length > 0 ? (
                    <EChart
                      option={apiKeyShareOption}
                      ariaLabel="API Key Token 使用占比环形图"
                      className="h-72 min-h-64 w-full"
                    />
                  ) : (
                    <ShareChartEmptyState />
                  )}
                </section>
              )}
            </div>
          </article>
          <article className={`${panelClass} grid gap-3 p-4`}>
            <ChartHeading title={allUsers ? "全历史用户消耗排行" : "全历史模型消耗排行"} />
            {(allUsers ? userRankPoints.length : modelPoints.length) > 0 ? (
              <EChart
                option={rankOption}
                ariaLabel={allUsers ? "用户 Token 消耗排行柱状图" : "模型 Token 消耗排行柱状图"}
              />
            ) : (
              <ChartEmptyState />
            )}
          </article>
        </div>
      ) : null}
    </section>
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

function ChartEmptyState() {
  return <div className="flex min-h-72 items-center justify-center px-5 text-center text-sm text-slate-500 dark:text-slate-400">暂无历史 Token 用量</div>;
}

function ShareChartEmptyState() {
  return <div className="flex min-h-64 items-center justify-center text-sm text-slate-500 dark:text-slate-400">暂无用量</div>;
}

function ContributionLegend({ darkMode }: { darkMode: boolean }) {
  const palette = CONTRIBUTION_PALETTES[darkMode ? "dark" : "light"];
  return (
    <div className="flex items-center gap-1 text-[11px] text-slate-500 dark:text-slate-400" aria-label="Token 活跃度颜色图例，从少到多">
      <span className="mr-0.5">少</span>
      {palette.map((color) => (
        <span
          key={color}
          className="h-3 w-3 rounded-[3px] border border-black/5 dark:border-white/5"
          style={{ backgroundColor: color }}
        />
      ))}
      <span className="ml-0.5">多</span>
    </div>
  );
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

const WEEKDAY_LABELS = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

interface ContributionChartDatum {
  value: [string, string, number];
  date: string;
  totalTokens: string;
  requestCount: string;
  activeRank: number | null;
  itemStyle: { color: string; opacity: number };
  label?: {
    show: boolean;
    formatter: string;
    textStyle: { color: string; fontSize: number; fontWeight: number };
  };
}

/**
 * 将后端连续 365 个自然日映射为 GitHub 风格的“周 × 星期”地形。
 * Z 轴使用 log10(token + 1)，既保留峰谷趋势，也避免单个异常高峰压扁其余日期；Tooltip
 * 始终展示原始精确值，不把视觉变换误当成业务数据。
 */
function buildContributionOption(points: UsageDailyPoint[], darkMode: boolean): EChartOption {
  const colors = chartThemeColors(darkMode);
  const parsedPoints = points
    .map((point) => ({ point, date: parseUsageDate(point.date) }))
    .filter((entry): entry is { point: UsageDailyPoint; date: Date } => entry.date !== null);
  const leadingDays = parsedPoints.length > 0 ? mondayBasedWeekday(parsedPoints[0].date) : 0;
  const weekCount = Math.max(1, Math.ceil((leadingDays + parsedPoints.length) / 7));
  const weekCategories = Array.from({ length: weekCount }, (_, index) => String(index));
  const monthLabels = Array.from({ length: weekCount }, () => "");
  const tokenValues = parsedPoints.map(({ point }) => parseTokenCount(point.total_tokens));
  const heightValues = tokenValues.map(logarithmicTokenHeight);
  const maxHeight = Math.max(0, ...heightValues);
  const palette = CONTRIBUTION_PALETTES[darkMode ? "dark" : "light"];
  const activeRanks = [...parsedPoints]
    .filter(({ point }) => parseTokenCount(point.total_tokens) > 0n)
    .sort((left, right) => compareTokenCounts(right.point.total_tokens, left.point.total_tokens));
  const rankByDate = new Map(activeRanks.map(({ point }, index) => [point.date, index + 1]));

  const data: ContributionChartDatum[] = parsedPoints.map(({ point, date }, index) => {
    const weekdayIndex = mondayBasedWeekday(date);
    const weekIndex = Math.floor((leadingDays + index) / 7);
    const month = date.getUTCMonth() + 1;
    if (index === 0 || date.getUTCDate() === 1) {
      monthLabels[weekIndex] = month === 1 ? `${date.getUTCFullYear()}年1月` : `${month}月`;
    }
    const tokenValue = tokenValues[index];
    const height = heightValues[index];
    const colorLevel = contributionColorLevel(tokenValue, height, maxHeight);
    const isToday = index === parsedPoints.length - 1;
    return {
      value: [String(weekIndex), WEEKDAY_LABELS[weekdayIndex], height],
      date: point.date,
      totalTokens: point.total_tokens,
      requestCount: point.request_count,
      activeRank: rankByDate.get(point.date) ?? null,
      itemStyle: { color: palette[colorLevel], opacity: isToday ? 1 : 0.94 },
      ...(isToday
        ? {
            label: {
              show: true,
              formatter: "今天",
              textStyle: {
                color: darkMode ? "#f8fafc" : "#334155",
                fontSize: 11,
                fontWeight: 600,
              },
            },
          }
        : {}),
    };
  });

  return {
    aria: { enabled: true },
    animationDuration: 650,
    textStyle: { fontFamily: CHART_FONT_FAMILY },
    tooltip: {
      ...chartTooltip(darkMode),
      trigger: "item",
      formatter: (parameter: { data?: ContributionChartDatum }) => {
        const datum = parameter.data;
        if (!datum) {
          return "";
        }
        const rank = datum.activeRank === null ? "" : `<br/>活跃日排名：第 ${datum.activeRank} 名`;
        return `${formatContributionDate(datum.date)}<br/>Token：${formatTokenCount(datum.totalTokens)}<br/>请求：${formatTokenCount(datum.requestCount)} 次${rank}`;
      },
    },
    grid3D: {
      left: 24,
      right: 24,
      top: 0,
      bottom: 12,
      boxWidth: 220,
      boxDepth: 46,
      boxHeight: 72,
      environment: darkMode ? "#0f172a" : "#ffffff",
      axisPointer: { show: false },
      viewControl: {
        projection: "perspective",
        alpha: 28,
        beta: -32,
        distance: 255,
        minDistance: 165,
        maxDistance: 360,
        minAlpha: 12,
        maxAlpha: 65,
        minBeta: -70,
        maxBeta: 20,
        rotateSensitivity: 0.75,
        zoomSensitivity: 0.8,
        panSensitivity: 0,
        autoRotate: false,
      },
      light: {
        main: { intensity: darkMode ? 1.3 : 1.15, alpha: 38, beta: -30, shadow: false },
        ambient: { intensity: darkMode ? 0.65 : 0.8 },
      },
    },
    xAxis3D: {
      type: "category",
      name: "月份 / 周",
      data: weekCategories,
      axisLabel: {
        interval: 0,
        formatter: (_value: string, index: number) => monthLabels[index] ?? "",
        textStyle: { color: colors.mutedText, fontSize: 10 },
      },
      axisLine: { lineStyle: { color: colors.axis, width: 1 } },
      axisTick: { show: false },
      splitLine: { show: false },
    },
    yAxis3D: {
      type: "category",
      name: "星期",
      data: WEEKDAY_LABELS,
      axisLabel: { textStyle: { color: colors.mutedText, fontSize: 10 } },
      axisLine: { lineStyle: { color: colors.axis, width: 1 } },
      axisTick: { show: false },
      splitLine: { lineStyle: { color: darkMode ? "#334155" : "#e2e8f0", width: 1 } },
    },
    zAxis3D: {
      type: "value",
      name: "Token 强度（log）",
      min: 0,
      max: Math.max(1, Math.ceil(maxHeight * 1.15)),
      axisLabel: { show: false },
      axisLine: { lineStyle: { color: colors.axis, width: 1 } },
      axisTick: { show: false },
      splitLine: { lineStyle: { color: darkMode ? "#334155" : "#e2e8f0", width: 1 } },
    },
    series: [
      {
        name: "每日 Token",
        type: "bar3D",
        coordinateSystem: "cartesian3D",
        data,
        minHeight: 0.7,
        bevelSize: 0.12,
        bevelSmoothness: 2,
        shading: "lambert",
        emphasis: {
          label: { show: false },
          itemStyle: { opacity: 1 },
        },
      },
    ],
  };
}

function parseUsageDate(value: string) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) {
    return null;
  }
  const date = new Date(Date.UTC(Number(match[1]), Number(match[2]) - 1, Number(match[3])));
  return Number.isNaN(date.getTime()) ? null : date;
}

function mondayBasedWeekday(date: Date) {
  return (date.getUTCDay() + 6) % 7;
}

function logarithmicTokenHeight(tokens: bigint) {
  if (tokens <= 0n) {
    return 0;
  }
  return Math.log10(Number(tokens) + 1);
}

function contributionColorLevel(tokens: bigint, height: number, maxHeight: number) {
  if (tokens <= 0n || maxHeight <= 0) {
    return 0;
  }
  return Math.min(4, Math.max(1, Math.ceil((height / maxHeight) * 4)));
}

function compareTokenCounts(left: string, right: string) {
  const leftValue = parseTokenCount(left);
  const rightValue = parseTokenCount(right);
  return leftValue > rightValue ? 1 : leftValue < rightValue ? -1 : 0;
}

function formatContributionDate(value: string) {
  const date = parseUsageDate(value);
  if (!date) {
    return value;
  }
  return new Intl.DateTimeFormat("zh-CN", {
    timeZone: "UTC",
    year: "numeric",
    month: "long",
    day: "numeric",
    weekday: "long",
  }).format(date);
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

function buildRankOption(
  points: Array<{ name: string; total_tokens: string }>,
  palette: readonly string[],
  darkMode: boolean,
): EChartOption {
  const colors = chartThemeColors(darkMode);
  // 排行图只有一个 bar series，为每个条目显式分色。管理员传入用户数据，普通用户传入
  // 模型数据，两种角色复用完全一致的数值轴与 Tooltip 行为。
  const ordered = points
    .map((point, index) => ({ point, color: palette[index % palette.length] }))
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
      data: ordered.map(({ point }) => point.name),
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

function modelLabel(point: UsageModelPoint) {
  return point.provider === OTHER_MODEL_PROVIDER
    ? point.model
    : `${point.model} · ${point.provider}`;
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
