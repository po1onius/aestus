/**
 * Dashboard 的 Tailwind 视觉基元。
 *
 * 这里只复用完整的 utility class 组合，不封装业务结构，也不生成运行时 CSS。这样既能让
 * Tailwind 在构建期完整扫描 class，又能保证按钮、表单、表格等高频控件遵循同一套规范。
 */

export function cx(...classes: Array<string | false | null | undefined>) {
  return classes.filter(Boolean).join(" ");
}

const focusRing =
  "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-indigo-600/30 focus-visible:ring-offset-2 dark:focus-visible:ring-indigo-400/35 dark:focus-visible:ring-offset-slate-950";
const disabled = "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50";
const buttonCore = `inline-flex items-center justify-center gap-2 rounded-lg border font-semibold transition-colors ${focusRing} ${disabled}`;
const secondaryTone = "border-slate-300 bg-white text-slate-700 hover:border-slate-400 hover:bg-slate-50 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200 dark:hover:border-slate-600 dark:hover:bg-slate-800";
const dangerTone = "border-red-200 bg-white text-red-700 hover:border-red-300 hover:bg-red-50 dark:border-red-900 dark:bg-slate-900 dark:text-red-300 dark:hover:border-red-800 dark:hover:bg-red-950/40";

export const buttonSecondary = `${buttonCore} ${secondaryTone} min-h-9 px-3 py-2 text-sm`;
export const buttonPrimary = `${buttonCore} min-h-9 border-indigo-600 bg-indigo-600 px-3 py-2 text-sm text-white hover:border-indigo-700 hover:bg-indigo-700 dark:border-indigo-400 dark:bg-indigo-400 dark:text-slate-950 dark:hover:border-indigo-300 dark:hover:bg-indigo-300`;
export const buttonDangerSolid = `${buttonCore} min-h-9 border-red-700 bg-red-700 px-3 py-2 text-sm text-white hover:border-red-800 hover:bg-red-800`;
export const buttonSmall = `${buttonCore} ${secondaryTone} min-h-8 px-2.5 py-1.5 text-xs`;
export const buttonSmallDanger = `${buttonCore} ${dangerTone} min-h-8 px-2.5 py-1.5 text-xs`;
export const iconButton = `${buttonCore} ${secondaryTone} size-9 text-sm`;
export const iconButtonSmall = `${buttonCore} ${secondaryTone} size-8 text-xs`;

export const fieldStack = "grid gap-1.5";
export const fieldLabel = "text-sm font-medium text-slate-700 dark:text-slate-300";
export const requiredMark = "ml-1 text-red-600 dark:text-red-400";
export const fieldHelp = "text-xs leading-5 text-slate-500 dark:text-slate-400";
export const inputClass = `min-h-10 w-full rounded-lg border border-slate-300 bg-white px-3 py-2 text-sm text-slate-900 shadow-xs outline-none transition placeholder:text-slate-400 hover:border-slate-400 focus:border-indigo-600 focus:ring-3 focus:ring-indigo-600/12 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100 dark:placeholder:text-slate-600 dark:hover:border-slate-600 dark:focus:border-indigo-400 dark:focus:ring-indigo-400/18 ${disabled}`;
export const compactInputClass = `min-h-9 w-full rounded-lg border border-slate-300 bg-white px-3 py-1.5 text-sm text-slate-900 shadow-xs outline-none transition placeholder:text-slate-400 hover:border-slate-400 focus:border-indigo-600 focus:ring-3 focus:ring-indigo-600/12 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100 dark:placeholder:text-slate-600 dark:hover:border-slate-600 dark:focus:border-indigo-400 dark:focus:ring-indigo-400/18 ${disabled}`;
export const textareaClass = `${inputClass} min-h-24 resize-y leading-6`;

export const panelClass =
  "rounded-xl border border-slate-200/80 bg-white/95 shadow-[inset_0_1px_0_rgba(255,255,255,0.95),0_1px_2px_rgba(15,23,42,0.05),0_14px_32px_-18px_rgba(15,23,42,0.28)] backdrop-blur-sm dark:border-slate-800 dark:bg-slate-900/95 dark:shadow-[inset_0_1px_0_rgba(255,255,255,0.04),0_1px_2px_rgba(0,0,0,0.35),0_16px_36px_-18px_rgba(0,0,0,0.85)]";
export const panelHeaderClass =
  "flex flex-wrap items-start justify-between gap-4 border-b border-slate-200 px-5 py-4 dark:border-slate-800";
export const panelTitleClass = "text-base font-semibold tracking-tight text-slate-950 dark:text-slate-100";
export const panelDescriptionClass = "mt-1 text-sm text-slate-500 dark:text-slate-400";

export const metricGridClass = "grid grid-cols-2 gap-3 lg:grid-cols-4";
export const metricClass =
  "min-w-0 rounded-xl border px-4 py-3.5 shadow-[inset_0_1px_0_rgba(255,255,255,0.95),0_1px_2px_rgba(15,23,42,0.04),0_10px_24px_-18px_rgba(15,23,42,0.3)] dark:shadow-[inset_0_1px_0_rgba(255,255,255,0.04),0_1px_2px_rgba(0,0,0,0.35),0_12px_28px_-18px_rgba(0,0,0,0.9)]";

export const tableScrollClass = "w-full overflow-x-auto";
export const tableClass =
  "w-full table-fixed border-collapse text-left text-sm [&_th]:border-b [&_th]:border-slate-200 [&_th]:bg-slate-50/95 [&_th]:px-4 [&_th]:py-3 [&_th]:text-xs [&_th]:font-semibold [&_th]:uppercase [&_th]:tracking-wide [&_th]:text-slate-500 [&_td]:border-b [&_td]:border-slate-100 [&_td]:px-4 [&_td]:py-3.5 [&_td]:align-top [&_tbody_tr:last-child_td]:border-b-0 dark:[&_th]:border-slate-800 dark:[&_th]:bg-slate-950/80 dark:[&_th]:text-slate-400 dark:[&_td]:border-slate-800/80";
export const disabledRowClass = "[&>td]:bg-slate-50 [&>td]:text-slate-500 dark:[&>td]:bg-slate-950/70 dark:[&>td]:text-slate-500";
export const entryStackClass = "grid min-w-0 gap-1";
export const entryTitleClass = "truncate text-sm font-semibold text-slate-900 dark:text-slate-100";
export const cellMainClass = "truncate text-sm font-medium leading-5 text-slate-800 dark:text-slate-200";
export const cellNoteClass = "truncate text-xs leading-5 text-slate-500 dark:text-slate-400";
export const cellWrapClass = "whitespace-normal break-words text-sm leading-5 text-slate-700 dark:text-slate-300";
export const actionStackClass = "grid max-w-52 grid-cols-2 gap-2";

export const emptyStateClass =
  "flex min-h-56 items-center justify-center gap-3 px-6 py-12 text-center text-sm text-slate-500 dark:text-slate-400";

export const tabClass = `group/tab relative z-10 min-h-9 rounded-md px-3 py-2 text-sm font-semibold transition-colors duration-200 ${focusRing} ${disabled}`;
export const tabContentClass = "block transition-transform duration-200 group-hover/tab:-translate-y-px";
export const tabSelectedClass = "text-indigo-800 dark:text-indigo-300";
export const tabIdleClass = "text-slate-600 hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100";

export const statusBadgeBase =
  "inline-flex w-fit items-center rounded-full px-2.5 py-1 text-xs font-semibold ring-1 ring-inset";

export const spinnerClass = "animate-spin";
