import { ChevronLeft, ChevronRight } from "lucide-react";
import { iconButton } from "../lib/ui";

interface ListPagerProps {
  offset: number;
  limit: number;
  itemCount: number;
  nextOffset: number | null;
  loading: boolean;
  label: string;
  onPageChange: (offset: number) => void;
}

/** Dashboard 普通管理列表共享的有界 offset 分页控件。 */
export function ListPager(props: ListPagerProps) {
  const page = Math.floor(props.offset / props.limit) + 1;
  return (
    <div className="flex flex-wrap items-center justify-end gap-3 border-t border-slate-200 px-4 py-3 text-xs text-slate-500 dark:border-slate-800 dark:text-slate-400">
      <span aria-live="polite">
        第 {page} 页 · 当前页 {props.itemCount} {props.label}
      </span>
      <div className="flex gap-1.5" aria-label={`${props.label}分页`}>
        <button
          type="button"
          className={iconButton}
          disabled={props.loading || props.offset === 0}
          onClick={() => props.onPageChange(Math.max(0, props.offset - props.limit))}
          title="上一页"
          aria-label="上一页"
        >
          <ChevronLeft size={18} />
        </button>
        <button
          type="button"
          className={iconButton}
          disabled={props.loading || props.nextOffset === null}
          onClick={() => props.nextOffset !== null && props.onPageChange(props.nextOffset)}
          title="下一页"
          aria-label="下一页"
        >
          <ChevronRight size={18} />
        </button>
      </div>
    </div>
  );
}
