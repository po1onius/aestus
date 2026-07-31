import type { ReactNode } from "react";
import { cx } from "../lib/ui";

interface SlidingTabListProps {
  count: 2 | 3;
  selectedIndex: number;
  ariaLabel: string;
  role?: "tablist" | "group";
  className?: string;
  children: ReactNode;
}

const columnClasses = {
  2: "grid-cols-2",
  3: "grid-cols-3",
} as const;

const indicatorWidthClasses = {
  2: "w-[calc((100%-0.5rem)/2)]",
  3: "w-[calc((100%-0.5rem)/3)]",
} as const;

const indicatorPositionClasses = [
  "translate-x-0",
  "translate-x-full",
  "translate-x-[200%]",
] as const;

/**
 * 两项或三项分段选择器共用的滑动光标。
 * 光标独立于按钮内容移动，既不会改变原有点击逻辑，也能避免每个页面重复维护动画布局。
 */
export function SlidingTabList({
  count,
  selectedIndex,
  ariaLabel,
  role = "tablist",
  className,
  children,
}: SlidingTabListProps) {
  const hasSelection = selectedIndex >= 0 && selectedIndex < count;
  const positionClass = hasSelection
    ? indicatorPositionClasses[selectedIndex]
    : indicatorPositionClasses[0];

  return (
    <div
      className={cx(
        "relative grid rounded-lg border border-slate-200 bg-slate-100 p-1 dark:border-slate-700 dark:bg-slate-950/80",
        columnClasses[count],
        className,
      )}
      role={role}
      aria-label={ariaLabel}
    >
      <span
        className={cx(
          "pointer-events-none absolute inset-y-1 left-1 rounded-md bg-white shadow-xs ring-1 ring-slate-200 transition-[translate,opacity] duration-300 ease-out dark:bg-slate-800 dark:ring-slate-700",
          indicatorWidthClasses[count],
          positionClass,
          hasSelection ? "opacity-100" : "opacity-0",
        )}
        aria-hidden="true"
      />
      {children}
    </div>
  );
}
