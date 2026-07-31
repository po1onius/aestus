import { CalendarDays } from "lucide-react";
import { useRef } from "react";
import { cx, iconButtonSmall } from "../lib/ui";

interface DatePickerInputProps {
  value: string;
  min?: string;
  max?: string;
  disabled?: boolean;
  className?: string;
  ariaLabel?: string;
  onChange: (value: string) => void;
}

/**
 * Dashboard 共用的只选日期控件。
 *
 * 日期以普通文本展示，只有独立日历按钮可打开选择器。真正的原生 date input 不参与布局、
 * 指针和键盘导航，只作为 showPicker() 的持久值载体，因此不会再出现年月日片段的文本选中态。
 */
export function DatePickerInput({
  value,
  min,
  max,
  disabled = false,
  className,
  ariaLabel = "选择日期",
  onChange,
}: DatePickerInputProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const displayValue = value || "未选择";

  return (
    <span className={cx("relative inline-flex w-fit min-w-0 items-center gap-2", className)}>
      <time
        className={cx(
          "whitespace-nowrap text-sm leading-5 text-slate-700 dark:text-slate-300",
          disabled && "text-slate-400 dark:text-slate-500",
        )}
        dateTime={value || undefined}
      >
        {displayValue}
      </time>
      <button
        className={iconButtonSmall}
        type="button"
        disabled={disabled}
        title={ariaLabel}
        aria-label={`${ariaLabel}，当前日期 ${displayValue}`}
        onClick={() => inputRef.current?.showPicker()}
      >
        <CalendarDays size={16} aria-hidden="true" />
      </button>
      {/* 原生控件只接收 picker 结果，不进入可见布局或可交互焦点序列。按钮同步调用
       * showPicker()，既保留浏览器原生日期面板，也把唯一可见交互入口限定为日历图标。 */}
      <input
        ref={inputRef}
        className="pointer-events-none absolute right-0 bottom-0 size-px opacity-0"
        type="date"
        value={value}
        min={min}
        max={max}
        disabled={disabled}
        tabIndex={-1}
        aria-hidden="true"
        onChange={(event) => {
          if (event.currentTarget.value) {
            onChange(event.currentTarget.value);
          }
        }}
      />
    </span>
  );
}
