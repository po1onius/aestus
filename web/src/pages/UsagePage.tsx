import { Loader2 } from "lucide-react";
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
// ECharts 热力图会让色块铺满类目槽位；使用面板底色描边把色块向内收缩，同时形成稳定间距。
const CONTRIBUTION_CELL_GAP = 3.5;
const CONTRIBUTION_CELL_RADIUS = 3;
const OTHER_MODEL_PROVIDER = "all";
const OTHER_MODEL_NAME = "其他模型";
const OTHER_API_KEY_NAME = "其他 API Key";
const USAGE_PERIODS = [
  { value: "year", label: "年", days: 365 },
  { value: "month", label: "月", days: 30 },
  { value: "week", label: "周", days: 7 },
] as const;
type UsagePeriod = (typeof USAGE_PERIODS)[number]["value"];

export function UsagePage({
  theme,
  usage,
  loading,
}: UsagePageProps) {
  const darkMode = theme === "dark";
  const [usagePeriod, setUsagePeriod] = useState<UsagePeriod>("year");
  const modelPoints = useMemo(() => compactModelPoints(usage?.models ?? []), [usage]);
  const apiKeyPoints = useMemo(() => compactApiKeyPoints(usage?.api_keys ?? []), [usage]);
  const userPoints = useMemo(() => usage?.users ?? [], [usage]);
  // 平台管理员排行展示真实的前八名租户，不把剩余租户汇总成一个会干扰名次的“其他租户”。
  const tenantRankPoints = useMemo(() => (usage?.tenants ?? []).slice(0, 8), [usage]);
  const usageScope = usage?.scope ?? "current_user";
  const allUsers = usageScope === "all_users";
  const tenantUsers = usageScope === "tenant";
  const usageActivityOption = useMemo(
    () => {
      const daily = usage?.daily ?? [];
      if (usagePeriod === "year") {
        return buildContributionOption(daily, darkMode);
      }
      const days = USAGE_PERIODS.find((period) => period.value === usagePeriod)?.days ?? 7;
      return buildDailyBarOption(daily.slice(-days), darkMode);
    },
    [darkMode, usage, usagePeriod],
  );
  const modelShareOption = useMemo(
    () => buildModelShareOption(modelPoints, darkMode),
    [darkMode, modelPoints],
  );
  const apiKeyShareOption = useMemo(
    () => buildConsumerShareOption(apiKeyPoints, "API Key Token", darkMode),
    [apiKeyPoints, darkMode],
  );
  const userShareOption = useMemo(
    () =>
      buildConsumerShareOption(
        userPoints.map((point) => ({
          name: point.username,
          total_tokens: point.total_tokens,
        })),
        "用户 Token",
        darkMode,
      ),
    [darkMode, userPoints],
  );
  const rankOption = useMemo(
    () =>
      allUsers
        ? buildRankOption(
            tenantRankPoints.map((point) => ({
              name: point.tenant_name,
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
    [allUsers, darkMode, modelPoints, tenantRankPoints],
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
              title={<UsagePeriodTabs value={usagePeriod} onChange={setUsagePeriod} />}
              controls={
                usagePeriod === "year" ? <ContributionLegend darkMode={darkMode} /> : undefined
              }
            />
            <EChart
              option={usageActivityOption}
              ariaLabel={
                usagePeriod === "year"
                  ? "最近 365 天每日 Token 用量贡献热力图"
                  : usagePeriod === "month"
                    ? "最近 30 天每日 Token 用量柱状图"
                    : "最近 7 天每日 Token 用量柱状图"
              }
              className={usagePeriod === "year" ? "h-56 min-h-52 w-full" : "h-72 min-h-64 w-full"}
            />
          </article>
          <article className={`${panelClass} grid gap-3 p-4`}>
            <ChartHeading title={tenantUsers ? "全历史租户用户消耗占比" : "全历史消耗占比"} />
            {tenantUsers ? (
              <section className="min-w-0">
                {userPoints.length > 0 ? (
                  <EChart
                    option={userShareOption}
                    ariaLabel="租户用户 Token 消耗占比环形图"
                    className="h-72 min-h-64 w-full"
                  />
                ) : (
                  <ShareChartEmptyState />
                )}
              </section>
            ) : (
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
            )}
          </article>
          <article className={`${panelClass} grid gap-3 p-4`}>
            <ChartHeading title={allUsers ? "全历史租户消耗排行" : "全历史模型消耗排行"} />
            {(allUsers ? tenantRankPoints.length : modelPoints.length) > 0 ? (
              <EChart
                option={rankOption}
                ariaLabel={allUsers ? "租户 Token 消耗排行柱状图" : "模型 Token 消耗排行柱状图"}
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
  title: ReactNode;
  controls?: ReactNode;
}) {
  return (
    <div className="flex min-h-8 flex-wrap items-center justify-between gap-3">
      {typeof title === "string" ? (
        <h2 className="text-sm font-semibold text-slate-900 dark:text-slate-100">{title}</h2>
      ) : (
        title
      )}
      {controls && (
        <div className="flex items-center gap-2 text-slate-500 dark:text-slate-400">
          {controls}
        </div>
      )}
    </div>
  );
}

function UsagePeriodTabs({
  value,
  onChange,
}: {
  value: UsagePeriod;
  onChange: (period: UsagePeriod) => void;
}) {
  return (
    <div
      className="inline-flex rounded-lg bg-slate-100 p-1 dark:bg-slate-800"
      role="tablist"
      aria-label="Token 用量统计周期"
    >
      {USAGE_PERIODS.map((period) => {
        const selected = value === period.value;
        return (
          <button
            key={period.value}
            type="button"
            role="tab"
            aria-selected={selected}
            className={`min-w-11 rounded-md px-3 py-1.5 text-xs font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-600/30 dark:focus-visible:ring-indigo-400/35 ${
              selected
                ? "bg-white text-indigo-700 shadow-sm dark:bg-slate-950 dark:text-indigo-300"
                : "text-slate-500 hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100"
            }`}
            onClick={() => onChange(period.value)}
          >
            {period.label}
          </button>
        );
      })}
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
  value: [number, number, number];
  date: string;
  totalTokens: string;
  requestCount: string;
  itemStyle: {
    color: string;
    borderColor: string;
    borderWidth: number;
    borderRadius: number;
  };
}

interface DailyBarChartDatum {
  value: number;
  date: string;
  totalTokens: string;
  requestCount: string;
  itemStyle: { color: string; borderRadius: [number, number, number, number] };
}

/**
 * 将后端连续 365 个自然日映射为 GitHub 风格的“周 × 星期”二维贡献网格。
 * 色阶使用相对于年度最小活跃日的对数比例，避免单个异常高峰吞掉常规用量差异；Tooltip
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
  const activeTokenValues = tokenValues.filter((tokens) => tokens > 0n);
  const minimumActiveTokens = activeTokenValues.reduce<bigint | null>(
    (minimum, tokens) => (minimum === null || tokens < minimum ? tokens : minimum),
    null,
  );
  const intensityValues = tokenValues.map((tokens) =>
    relativeLogarithmicTokenIntensity(tokens, minimumActiveTokens),
  );
  const maxIntensity = Math.max(0, ...intensityValues);
  const palette = CONTRIBUTION_PALETTES[darkMode ? "dark" : "light"];
  const cellGapColor = darkMode ? "#131820" : "#ffffff";

  const data: ContributionChartDatum[] = parsedPoints.map(({ point, date }, index) => {
    const weekdayIndex = mondayBasedWeekday(date);
    const weekIndex = Math.floor((leadingDays + index) / 7);
    const month = date.getUTCMonth() + 1;
    if (index === 0 || date.getUTCDate() === 1) {
      monthLabels[weekIndex] = month === 1 ? `${date.getUTCFullYear()}年1月` : `${month}月`;
    }
    const tokenValue = tokenValues[index];
    const intensity = intensityValues[index];
    const colorLevel = contributionColorLevel(tokenValue, intensity, maxIntensity);
    const isToday = index === parsedPoints.length - 1;
    return {
      value: [weekIndex, weekdayIndex, intensity],
      date: point.date,
      totalTokens: point.total_tokens,
      requestCount: point.request_count,
      itemStyle: {
        color: palette[colorLevel],
        // 今天使用主题强调色描边，既能定位当前日期，也不会覆盖 GitHub 风格的用量色阶。
        borderColor: isToday
          ? darkMode
            ? "#fb923c"
            : "#f97316"
          : cellGapColor,
        borderWidth: isToday ? 1.5 : CONTRIBUTION_CELL_GAP,
        borderRadius: CONTRIBUTION_CELL_RADIUS,
      },
    };
  });

  return {
    aria: { enabled: true },
    animationDuration: 650,
    tooltip: {
      ...chartTooltip(darkMode),
      trigger: "item",
      formatter: (parameter: { data?: ContributionChartDatum }) => {
        const datum = parameter.data;
        if (!datum) {
          return "";
        }
        return `${formatContributionDate(datum.date)}<br/>Token：${formatTokenCount(datum.totalTokens)}<br/>请求：${formatTokenCount(datum.requestCount)} 次`;
      },
    },
    grid: {
      left: 42,
      right: 12,
      top: 34,
      height: 140,
    },
    xAxis: {
      type: "category",
      data: weekCategories,
      position: "top",
      axisLabel: {
        interval: 0,
        formatter: (_value: string, index: number) => monthLabels[index] ?? "",
        color: colors.mutedText,
        fontSize: 10,
        margin: 10,
      },
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { show: false },
    },
    yAxis: {
      type: "category",
      data: WEEKDAY_LABELS,
      inverse: true,
      axisLabel: {
        color: colors.mutedText,
        fontSize: 10,
        formatter: (value: string) => (["周一", "周三", "周五"].includes(value) ? value : ""),
      },
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { show: false },
    },
    series: [
      {
        name: "每日 Token",
        type: "heatmap",
        data,
        progressive: 0,
        itemStyle: {
          color: palette[0],
          borderColor: cellGapColor,
          borderWidth: CONTRIBUTION_CELL_GAP,
          borderRadius: CONTRIBUTION_CELL_RADIUS,
        },
        emphasis: {
          itemStyle: {
            borderColor: darkMode ? "#e2e8f0" : "#334155",
            borderWidth: CONTRIBUTION_CELL_GAP,
          },
        },
      },
    ],
  };
}

/**
 * 月、周视图直接截取年度接口中的连续日数据，不发起新的请求。柱高使用原始 Token 数量，
 * 色阶继续沿用年度贡献图的相对对数强度，从而保持三个周期之间一致的视觉语义。
 */
function buildDailyBarOption(points: UsageDailyPoint[], darkMode: boolean): EChartOption {
  const colors = chartThemeColors(darkMode);
  const palette = CONTRIBUTION_PALETTES[darkMode ? "dark" : "light"];
  const tokenValues = points.map((point) => parseTokenCount(point.total_tokens));
  const minimumActiveTokens = tokenValues.reduce<bigint | null>(
    (minimum, tokens) =>
      tokens > 0n && (minimum === null || tokens < minimum) ? tokens : minimum,
    null,
  );
  const intensityValues = tokenValues.map((tokens) =>
    relativeLogarithmicTokenIntensity(tokens, minimumActiveTokens),
  );
  const maxIntensity = Math.max(0, ...intensityValues);
  const data: DailyBarChartDatum[] = points.map((point, index) => ({
    value: chartValue(point.total_tokens),
    date: point.date,
    totalTokens: point.total_tokens,
    requestCount: point.request_count,
    itemStyle: {
      color: palette[
        contributionColorLevel(tokenValues[index], intensityValues[index], maxIntensity)
      ],
      borderRadius: [4, 4, 0, 0],
    },
  }));

  return {
    aria: { enabled: true },
    animationDuration: 350,
    grid: { top: 18, right: 20, bottom: 42, left: 68 },
    tooltip: {
      ...chartTooltip(darkMode),
      trigger: "item",
      formatter: (parameter: { data?: DailyBarChartDatum }) => {
        const datum = parameter.data;
        if (!datum) {
          return "";
        }
        return `${formatContributionDate(datum.date)}<br/>Token：${formatTokenCount(datum.totalTokens)}<br/>请求：${formatTokenCount(datum.requestCount)} 次`;
      },
    },
    xAxis: {
      type: "category",
      data: points.map((point) => point.date),
      axisLabel: {
        color: colors.mutedText,
        interval: points.length <= 7 ? 0 : 4,
        formatter: (value: string) => formatShortUsageDate(value),
      },
      axisLine: { lineStyle: { color: colors.axis } },
      axisTick: { show: false },
    },
    yAxis: {
      type: "value",
      minInterval: 1,
      axisLabel: {
        color: colors.mutedText,
        formatter: (value: number) => chartNumberFormatter.format(value),
      },
      splitLine: { lineStyle: { color: colors.grid } },
    },
    series: [
      {
        name: "每日 Token",
        type: "bar",
        data,
        barMaxWidth: points.length <= 7 ? 52 : 22,
        showBackground: true,
        backgroundStyle: {
          color: darkMode ? "rgba(39, 49, 61, 0.45)" : "rgba(229, 231, 235, 0.55)",
          borderRadius: [4, 4, 0, 0],
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

function relativeLogarithmicTokenIntensity(tokens: bigint, minimumActiveTokens: bigint | null) {
  if (tokens <= 0n || minimumActiveTokens === null) {
    return 0;
  }

  // 与年度最小活跃日计算比值，可以在保留对数抗极值能力的同时，让常规用量分布到多个
  // 绿色等级；零用量始终独占灰色等级，不会与最低活跃日混淆。
  return Math.log10(Number(tokens) / Number(minimumActiveTokens) + 1);
}

function contributionColorLevel(tokens: bigint, intensity: number, maxIntensity: number) {
  if (tokens <= 0n || maxIntensity <= 0) {
    return 0;
  }
  return Math.min(4, Math.max(1, Math.ceil((intensity / maxIntensity) * 4)));
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

function formatShortUsageDate(value: string) {
  const date = parseUsageDate(value);
  return date ? `${date.getUTCMonth() + 1}/${date.getUTCDate()}` : value;
}

function buildModelShareOption(points: UsageModelPoint[], darkMode: boolean): EChartOption {
  const colors = chartThemeColors(darkMode);
  return {
    aria: { enabled: true },
    animationDuration: 350,
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
